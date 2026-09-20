# JadeDB — guía LSM (terminología + diseño + código)

Guía de estudio del motor. Objetivo: **entender cada pieza lo bastante bien como para diseñar el próximo proyecto** (otro engine, un cache, un pipeline durable) sin magia.

Cada sección enlaza conceptos con archivos de este repo.  
Grafo del código: [`graphify-out/graph.html`](../graphify-out/graph.html).

### Cómo usar esta guía

1. Leé las secciones **1–3** (problema, amplificaciones, glosario) de una sentada.  
2. Después andá **4 → 12** con el código abierto (`src/…`).  
3. Cerrá con **14** (entrevista) y **17** (plan de práctica / próximos proyectos).  
4. Cuando algo “haga clic” (como bloom vs Valkey), anotalo en la sección **18**.

---

## 1. El problema que resuelve un LSM

Los discos (y SSDs bajo carga aleatoria) son malos en **escrituras aleatorias** y buenos en **secuenciales**.

Un **B-tree** actualiza *in place*: buscar página → leer → modificar → reescribir. Eso son seeks y una página completa de I/O por escritura pequeña.

Un **LSM** (Log-Structured Merge-tree) da la vuelta: **nunca modifica un archivo inmutable en disco**. Solo agrega. Las escrituras van a memoria; cuando hay suficiente, se vuelcan en una pasada secuencial (SSTable). El costo: las lecturas se complican porque una clave puede vivir en varios sitios y hay que saber cuál versión es la más nueva.

Todo el diseño es administrar ese trade-off.

---

## 2. Las tres amplificaciones

No puedes optimizar las tres a la vez. Cada decisión de Jade es un punto en este espacio.

| Amplificación | Pregunta | En Jade |
|---|---|---|
| **Write amplification (WA)** | ¿Cuántos bytes escribís en disco por cada byte del usuario? | Compaction leveled reescribe datos al bajar de nivel |
| **Read amplification (RA)** | ¿Cuántos lugares consultás para un `get`? | Mem + imms + L0 (varios) + a lo sumo 1 SST por nivel ≥1 + bloom |
| **Space amplification (SA)** | ¿Cuánto espacio de más por versiones viejas y tombstones? | Se limpia en compaction; snapshots retrasan el GC |

**Regla de entrevista:** leveled → mejor RA/SA, peor WA. Tiered/universal → mejor WA, peor RA/SA.

**Frase útil:** la escritura “rápida” del LSM es un **préstamo**. El cobro llega en flush + compaction. El WAL es el seguro del préstamo.

---

## 3. Glosario (terminología)

### Estructuras

| Término | Qué es | Dónde en Jade |
|---|---|---|
| **Memtable** | Mapa ordenado en RAM que absorbe writes | `src/memtable.rs` (`BTreeMap`) |
| **Immutable memtable (imm)** | Memtable congelada esperando flush | `Inner.imms` en `src/db.rs` |
| **WAL** | Log append-only en disco para recovery | `src/wal.rs`, archivo `CURRENT.log` |
| **SSTable / SST** | Archivo inmutable con **pares clave→valor** ordenados | `src/sstable/` |
| **Block** | Trozo de datos dentro de un SST | `src/sstable/block.rs` |
| **Index block** | Índice *dentro* del SST: separator → offset/len | `IndexBlock` |
| **Bloom filter** | Probabilístico por SST: “seguro que no” o “quizá sí” | `src/sstable/bloom.rs` |
| **Block cache** | Cache en memoria de bloques ya leídos | `BlockCache` en `reader.rs` |
| **Manifest** | Log de qué SSTs existen en cada nivel | `src/manifest.rs`, archivo `MANIFEST` |
| **Version** | Snapshot en memoria de la jerarquía de niveles | `src/version.rs` |
| **Level / L0, L1, …** | Capas del árbol; L0 puede solaparse | `Version.levels` |
| **Compaction** | Merge de SSTs: baja niveles, limpia basura | `src/compact.rs` |
| **Merge iterator** | Une varios streams ordenados eligiendo la versión nueva | `src/iter.rs` |
| **Snapshot** | Lectura con `seq` máximo fijo (MVCC simple) | `src/snapshot.rs` |

### Registros y versiones

| Término | Qué es |
|---|---|
| **User key** | La clave que escribe el cliente (`[u8]`) |
| **Sequence number / seq** | Contador monótono por write; más alto = más nuevo |
| **Internal key** | `(user_key, seq, kind)` — clave de ordenamiento real |
| **ValueKind::Put / Delete** | Valor normal vs **tombstone** (borrado lógico) |
| **Tombstone** | Registro “esta clave está muerta”; no borra bytes viejos aún |
| **Watermark** | `seq` del snapshot más viejo activo; limita qué se puede GC |

### Operaciones y durabilidad

| Término | Qué es |
|---|---|
| **Flush** | Escribir memtable inmutable → SST L0 |
| **Freeze** | Convertir mem activa en imm y abrir mem nueva |
| **fsync / `sync_all`** | Forzar datos del page cache al medio durable |
| **Group commit** | Fsync por lotes (`FsyncMode::Batch`), no por cada write |
| **Write stall** | Frenar writers cuando flush/compaction no dan abasto |
| **Orphan SST** | Archivo `.sst` en disco no mencionado en el manifest |
| **Recovery** | Al `open`: leer MANIFEST + replay WAL |
| **Falso positivo (FP)** | Bloom dice “quizá” y la clave no está — solo cuesta I/O |
| **Falso negativo (FN)** | Bloom dice “no” y la clave sí está — **bug de corrección** |

### Amplificación y estrategias

| Término | Qué es |
|---|---|
| **Leveled compaction** | Cada nivel ~10× el anterior; L≥1 sin overlap de rangos |
| **Size-tiered / universal** | Agrupa SSTs de tamaño similar; menos WA, más overlap |
| **L0** | Nivel “joven”: SSTs pueden solaparse → hay que mirar varios |

---

## 4. ¿Qué es un SST? (bien claro)

**SST = Sorted String Table** (a veces “Sorted Static Table”).

Es un **archivo en disco, inmutable**, que guarda **pares clave→valor** (y tombstones), **ya ordenados**.

No es “solo las claves”. Cada entrada es algo como `(internal_key, value)`.

### Analogía

- **Memtable** = borrador ordenado en RAM.  
- **SST** = fotocopia ordenada que mandás al archivo.  
- Si mañana “corregís” una clave, **no tachás** la fotocopia: imprimís una página nueva; más tarde la compaction junta fotocopias y tira las viejas.

### No confundir con índice secundario

El SST **no es** un índice aparte tipo “index key → value key”.  
**Es la tabla primaria** en forma de run ordenado.

Dentro del archivo sí hay un índice **de navegación**:

| Pieza | Rol |
|---|---|
| **Data blocks** | Entradas reales: key + value (o tombstone) |
| **Index block** | “Para esta zona de claves, andá al offset X” |
| **Bloom** | “¿Vale la pena ni abrir este archivo?” |

Analogía libro: capítulos ordenados (data) + índice al final (index block) + “¿aparece esta palabra?” (bloom).

### Relación con L0

El flush siempre crea un SST en **L0**. Varios L0 pueden **solaparse** en rangos → un `get` a veces mira más de uno.  
En L1+ (leveled) los rangos **no** se solapan → a lo sumo un candidato por nivel.

Archivos: `000003.sst`, etc. Código: `src/sstable/`.

---

## 5. Internal key — orden que lo gobierna todo

Orden: **user_key ASC**, luego **seq DESC** (más nuevo primero).

```text
put("a", v1)  seq=1
put("a", v2)  seq=2
delete("a")   seq=3   → tombstone

Al iterar "a": seq=3 (Delete) gana → get = None
Snapshot con max_seq=2: ve v2
```

Implementación: `src/key.rs` (`InternalKey`, `ValueKind`, `encode`/`decode`).

En disco el tag se empaqueta como `(seq << 8) | kind` al final de la clave encodificada.

Por eso la versión nueva aparece primero en el `BTreeMap` y en los merges.

---

## 6. Camino de una escritura

```text
Cliente
  │
  ▼
put/delete ──► next_seq++ ──► InternalKey
  │
  ├─1─► WAL.append   (durabilidad)
  └─2─► memtable.put (visibilidad)
  │
  ▼
ack al cliente

si mem ≈ llena:
  wal.sync → freeze mem → push imm → notify bg_loop
```

Código: `Db::write` en `src/db.rs`.

**Orden WAL → mem:** si respondieras antes del WAL, un crash pierde datos confirmados.

### Stall

Si `imms >= 2` o L0 crece demasiado, el write duerme un poco y despierta al background (`stall_count`). Eso es presión de compaction hecha latencia p99.

### Fsync modes (`src/options.rs`)

| Modo | Comportamiento | Trade-off |
|---|---|---|
| `Always` | `sync_all` tras cada append WAL | Máxima durabilidad, lento |
| `Batch` | fsync cada ~256 KiB pendientes | Punto medio (producción típica) |
| `Never` | sin fsync | Rápido; inseguro ante corte de luz |

`write(2)` solo llega al page cache del OS. Sin fsync, un power loss puede perder “writes exitosos”.

Números de ejemplo en este repo (`jade bench`, 10k puts): Never ~117k ops/s, Batch ~111k, Always ~2.6k.

---

## 7. Borrar sin borrar (tombstones)

`delete(key)` escribe un registro Delete. Cuesta igual que un put.

Al leer: si el primer hallazgo (seq más alto visible) es Delete → `None`.

**Bug clásico:** descartar un tombstone en compaction mientras aún existe una versión más vieja de esa clave en un nivel inferior → **resurrección** del dato.

Jade solo dropea tombstones de forma agresiva cerca del último nivel y respecto al **watermark** de snapshots (`src/compact.rs`).

---

## 8. Camino de una lectura (`get`)

Orden de más nuevo a más viejo; **stop en el primer hallazgo**:

1. Memtable activa  
2. Imms (de más nueva a más vieja)  
3. L0 (todas las candidatas; se solapan)  
4. L1…Ln: a lo sumo **un** SST por nivel (rangos no solapados)

Código: `Db::get_with_seq` → `Version::get_sstables_for_key` → `SsTable::get`.

### Dentro de un SST

```text
rango smallest..largest?
  → bloom.may_contain?     (FN = bug; FP = solo I/O extra)
    → index.find_block
      → leer 1 data block (+ block cache)
        → buscar user_key con seq <= max_seq
```

### `scan`

Misma idea de versiones, pero con **merge iterator** sobre memtables + SSTs (`src/iter.rs`). Emite cada user key una vez (la más nueva visible).  
Nota: `scan` puede recorrer data blocks aunque el bloom diga “no” para un point lookup — son caminos distintos.

---

## 9. Bloom filters (a fondo)

### Idea

Un **bloom** es un bitmap chico **por SST**. Responde:

- **`false`** → la clave **seguro no está** → no abras el archivo  
- **`true`** → **quizá está** → seguí con índice + bloque  

| Error | ¿OK? |
|---|---|
| Falso positivo (“tal vez” y no estaba) | Sí — solo I/O extra |
| Falso negativo (“no” y sí estaba) | **Bug** — datos “desaparecen” en `get` |

Con ~**10 bits por clave** (`Options.bloom_bits_per_key`) el FP ronda ~1%.

### Ciclo de vida en Jade

1. **Al crear el SST** (`SsTableBuilder::finish`): `BloomFilter::build(user_keys, bits_per_key)` → se escribe en el archivo.  
2. **Al abrir** (`SsTable::open`): decode desde el footer.  
3. **En `get`**: `may_contain` antes de tocar data blocks.  

Los blooms de SST **viejos no se actualizan** cuando llega un put nuevo. La key nueva vive en **memtable/WAL** hasta el próximo flush, que crea **otro** SST con **otro** bloom. Eso es correcto.

### Cómo funciona por dentro (`src/sstable/bloom.rs`)

Para cada clave:

1. Dos hashes (`h1`, `h2`, crc32).  
2. Se prenden **k** bits: `h1 + i*h2` mod `nbits`.  

En `may_contain`: si **algún** bit de esos k está en 0 → no está. Si todos en 1 → tal vez.

**Bug real que tuvimos en Jade:** `may_contain` usaba `bits.len()*8` como `nbits` en vez del `nbits` del build → falsos negativos → `get` fallaba y `scan` no. Lección: build y probe deben compartir **exactamente** la misma aritmética.

### Bloom en Valkey/Redis vs bloom en SST

Mismo truco probabilístico; **distinto rol**:

| | Bloom en **Jade SST** | Bloom en **Valkey** (RedisBloom) |
|---|---|---|
| Rol | Acelerar `get` sobre un archivo inmutable | Estructura: “¿ya vi este item?” (`BF.ADD` / `BF.EXISTS`) |
| Cuándo nace | Al **cerrar** el SST | En cada add (filtro mutable en RAM) |
| “Refresh” | No hay: keys nuevas van a mem → nuevo SST | El filtro se muta; puede haber lag de **réplica**, scaling del filtro, o cache de miss |

Si en el trabajo viste “delay al refrescar” tras agregar data, casi nunca era “el algoritmo del bloom”. Solía ser:

1. Lectura contra **réplica** con lag  
2. **Scaling** del filtro al llenarse la capacidad  
3. **Cache** de “no existe” en la app  
4. Otro nodo / AZ  

Mejoras típicas: read-your-writes al primary, dimensionar `capacity`/`error_rate`, no cachear negativos agresivamente.

**Puente mental:** Valkey muta el filtro y pagás sync. Jade no muta blooms viejos; paga flush.

---

## 10. Flush: de memtable a SST

`flush_imms` (`src/db.rs`):

1. Iterar imm ordenada → `SsTableBuilder`  
2. Escribir archivo `NNNNNN.sst`  
3. `fsync` del archivo (según modo)  
4. Append al **MANIFEST** (`AddFile` L0)  
5. Incorporar SST a `Version`  
6. Quitar imm  
7. Si no queda estado volatile pendiente → **recrear WAL**

### Formato SST (Jade)

```text
[data blocks...][index block][bloom][footer]
footer: magic | data_end | index_off | index_len | bloom_off | bloom_len
```

Builder: `src/sstable/builder.rs`. Reader: `src/sstable/reader.rs`.

---

## 11. Crash a mitad de un flush (el checkpoint)

**Pregunta de entrevista:** ¿qué pasa si el proceso muere mientras se escribe el SST?

**Respuesta (con el orden correcto):** nada malo.

- El SST a medio escribir **nunca entró al MANIFEST** → **huérfano** (basura).  
- El WAL **sigue vivo** porque solo se descarta **después** de SST durable + registrado.  
- Recovery: MANIFEST + replay WAL → estado previo al crash.

**Invariante central de Jade:**

```text
escribir SST → fsync SST → append MANIFEST (+ fsync) → recién entonces descartar WAL
```

Violación → pérdida de datos o resurrección de deletes.

WAL truncado a mitad de un record: se corta limpio (`wal.rs`: si no alcanza el `len`, `break`).

---

## 12. Manifest — quién sabe qué archivos existen

No se confía en `readdir` como fuente de verdad.

Edits típicos:

- `AddFile { file_num, level, smallest, largest, size }`  
- `DeleteFile { level, file_num }`  
- `SetNextFileNum` / `SetLastSeq`

Tras compaction: append “agrega estos, quita aquellos” (+ fsync).

Sin manifest no hay forma segura de hacer visible una compaction.

---

## 13. Compaction — el corazón

Sin compaction: RA y SA crecen sin límite.

Proceso: leer inputs → merge ordenado → dropear versiones viejas / tombstones seguros → escribir SSTs nuevas → MANIFEST → borrar inputs.

### Leveled (Jade)

- L0: overlap OK; trigger por **número de archivos**.  
- L≥1: **sin** overlap de rangos; tamaño ~ `base * ratio^(level-1)`.  
- WA alta, RA baja.

### Size-tiered / universal

Agrupa por tamaño similar. Menos WA, más overlap / SA. (No es el default de Jade.)

### Stalls

Memtable se llena más rápido que flush+compaction → stalls. Medir `Db::stats().stall_count`.

---

## 14. Snapshots / MVCC (stretch)

`Db::snapshot()` fija `seq = next_seq`.  
`get_snapshot` usa `max_seq = snap.seq`.

`watermark()` = mínimo seq activo (o `u64::MAX`). Compaction lo usa para no GC’ear de más (simplificado en Jade).

---

## 15. Mapa mental → archivos

```text
API pública          lib.rs, db.rs
Clave/versión        key.rs
Memoria              memtable.rs
Durabilidad volatile wal.rs
Disco inmutable      sstable/*          ← SST = KV ordenados + index + bloom
Metadatos            manifest.rs, version.rs
Mantenimiento        compact.rs, db.rs::{flush_imms,maybe_compact,bg_loop}
Lectura rango        iter.rs
MVCC                 snapshot.rs
Knobs                options.rs
```

**God nodes** del grafo (más conectados): `InternalKey`, `Db`, `SsTable`, `MemTable`, `Version`, `Wal`, `Manifest`.

---

## 16. Checklist de entrevista

1. LSM vs B-tree y las 3 amplificaciones.  
2. Invariante WAL → mem → ack; flush SST → manifest → drop WAL.  
3. Qué es un SST (KV ordenados inmutables, no “solo keys”).  
4. L0 especial; bloom sin FN; FP aceptable.  
5. Leveled vs tiered; stalls.  
6. Tombstone + seq; resurrección.  
7. Bloom Valkey (mutable) vs bloom SST (por archivo).  
8. Un número tuyo: fsync Always/Batch/Never.

---

## 17. Plan de práctica → más proyectos

### Dominio Jade (esta semana / siguientes)

| Día | Hacer | Criterio “lo entiendo” |
|---|---|---|
| 1 | Guía §§1–5 + leer `key.rs`, `memtable.rs` | Explicás internal key sin mirar |
| 2 | §§6–8 + `db.rs` write/get | Dibujás write path de memoria |
| 3 | §§9–11 + `bloom.rs`, `wal.rs`, flush | Explicás crash mid-flush |
| 4 | §§12–14 + `compact.rs`, `manifest.rs` | Leveled vs tiered en 2 min |
| 5 | Benches + graphify + checklist §16 | Contás Jade en entrevista de 10 min |

Comandos:

```bash
cargo test
cargo run --bin jade -- bench always|batch|never
cargo bench
graphify update .   # tras cambiar código
```

### Ideas de proyectos siguientes (mismo ADN)

Cuando Jade te quede sólido, el siguiente repo puede ser **más chico y más profundo en una perilla**:

1. **Bitcask-style KV** — solo log + hash index en RAM (puente mental más simple).  
2. **Jade-lite tiered compaction** — misma API, otra estrategia; comparar WA/RA en benches.  
3. **Mini query layer** — encima de Jade: rangos + predicados, sin SQL completo.  
4. **Distributed toy** — un nodo Jade + raft/replica log (aprender consistencia, no reinventar RocksDB).  
5. **Observability** — métricas p99, stalls, WA expuestas en Prometheus.

Patrón: **un trade-off medible + README que lo explique** > muchas features.

---

## 18. Notas personales / “aha”

*(Completá vos al estudiar.)*

- SST = …  
- Bloom SST vs Valkey = …  
- Si el proceso muere mid-flush = …  
- Por qué L0 es especial = …  
- Mi punto en el espacio WA/RA/SA = …  

---

## 19. Lecturas

1. Esta guía.  
2. [Mini-LSM](https://skyzh.github.io/mini-lsm/)  
3. [LevelDB Implementation](https://github.com/google/leveldb/blob/main/doc/impl.md)  
4. [RocksDB Compaction](https://github.com/facebook/rocksdb/wiki/Compaction)  
5. O’Neil et al., *The Log-Structured Merge-Tree* (1996)  
6. [`graphify-out/GRAPH_REPORT.md`](../graphify-out/GRAPH_REPORT.md)
