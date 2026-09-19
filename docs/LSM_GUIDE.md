# JadeDB — guía LSM (terminología + diseño + código)

Guía de estudio del motor. Cada sección enlaza conceptos con archivos de este repo.
Para navegar el grafo del código: abre [`graphify-out/graph.html`](../graphify-out/graph.html).

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

---

## 3. Glosario (terminología)

### Estructuras

| Término | Qué es | Dónde en Jade |
|---|---|---|
| **Memtable** | Mapa ordenado en RAM que absorbe writes | `src/memtable.rs` (`BTreeMap`) |
| **Immutable memtable (imm)** | Memtable congelada esperando flush | `Inner.imms` en `src/db.rs` |
| **WAL** | Log append-only en disco para recovery | `src/wal.rs`, archivo `CURRENT.log` |
| **SSTable / SST** | Archivo inmutable, claves ordenadas | `src/sstable/` |
| **Block** | Trozo de datos dentro de un SST | `src/sstable/block.rs` |
| **Index block** | Mapa separator → offset/len de data blocks | `IndexBlock` |
| **Bloom filter** | Probabilístico: “seguro que no” o “quizá sí” | `src/sstable/bloom.rs` |
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
| **Orphan SST** | Archivo `.sst` en disco no mencionado en el manifest (basura tras crash) |
| **Recovery** | Al `open`: leer MANIFEST + replay WAL |

### Amplificación y estrategias

| Término | Qué es |
|---|---|
| **Leveled compaction** | Cada nivel ~10× el anterior; L≥1 sin overlap de rangos |
| **Size-tiered / universal** | Agrupa SSTs de tamaño similar; menos WA, más overlap |
| **L0** | Nivel “joven”: SSTs pueden solaparse → hay que mirar varios |

---

## 4. Internal key — orden que lo gobierna todo

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

---

## 5. Camino de una escritura

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

Si `imms >= 2` o L0 crece demasiado, el write duerme un poco y despierta al background (`stall_count`). Eso es presión de compaction hecha latencia.

### Fsync modes (`src/options.rs`)

| Modo | Comportamiento | Trade-off |
|---|---|---|
| `Always` | `sync_all` tras cada append WAL | Máxima durabilidad, lento |
| `Batch` | fsync cada ~256 KiB pendientes | Punto medio (producción típica) |
| `Never` | sin fsync | Rápido; inseguro ante corte de luz |

`write(2)` solo llega al page cache del OS. Sin fsync, un power loss puede perder “writes exitosos”.

---

## 6. Borrar sin borrar (tombstones)

`delete(key)` escribe un registro Delete. Cuesta igual que un put.

Al leer: si el primer hallazgo (seq más alto visible) es Delete → `None`.

**Bug clásico:** descartar un tombstone en compaction mientras aún existe una versión más vieja de esa clave en un nivel inferior → **resurrección** del dato.

Jade solo dropea tombstones de forma agresiva cerca del último nivel y respecto al **watermark** de snapshots (`src/compact.rs`).

---

## 7. Camino de una lectura (`get`)

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

Misma idea de versiones, pero con **merge iterator** sobre memtables + todos los SSTs (`src/iter.rs`). Emite cada user key una vez (la más nueva visible).

---

## 8. Flush: de memtable a SST

`flush_imms` (`src/db.rs`):

1. Iterar imm ordenada → `SsTableBuilder`  
2. Escribir archivo `NNNNNN.sst`  
3. `fsync` del archivo (según modo)  
4. Append al **MANIFEST** (`AddFile` L0)  
5. Incorporar SST a `Version`  
6. Quitar imm  
7. Si no queda estado volatile pendiente → **recrear WAL** (descartar log viejo)

### Formato SST (Jade)

```text
[data blocks...][index block][bloom][footer]
footer: magic | data_end | index_off | index_len | bloom_off | bloom_len
```

Builder: `src/sstable/builder.rs`. Reader: `src/sstable/reader.rs`.

---

## 9. Crash a mitad de un flush (el checkpoint)

**Pregunta de entrevista:** ¿qué pasa si el proceso muere mientras se escribe el SST?

**Respuesta (con el orden correcto):** nada malo.

- El SST a medio escribir **nunca entró al MANIFEST** → al arrancar es **huérfano** (ignorable / basura).  
- El WAL de esa memtable **sigue vivo** porque solo se descarta **después** de SST durable + registrado.  
- Recovery: `Manifest::recover` + `Wal::recover` → memtable reconstruida → estado previo al crash.

**Invariante central de Jade:**

```text
escribir SST → fsync SST → append MANIFEST (+ fsync) → recién entonces descartar WAL
```

Violación en cualquier punto → pérdida de datos o resurrección de deletes.

Al recovery, un WAL truncado a mitad de un record se corta limpio (`wal.rs`: si no alcanza el `len`, `break`).

---

## 10. Manifest — quién sabe qué archivos existen

No se confía en `readdir` como fuente de verdad.

El MANIFEST es un log de edits:

- `AddFile { file_num, level, smallest, largest, size }`  
- `DeleteFile { level, file_num }`  
- `SetNextFileNum` / `SetLastSeq`

Tras compaction: append “agrega estos, quita aquellos” de forma atómica a nivel de record (+ fsync del archivo).

Sin manifest no hay forma segura de hacer visible una compaction: huérfanos, duplicados o datos “desaparecidos”.

---

## 11. Compaction — el corazón

Sin compaction: RA y SA crecen sin límite.

Proceso: leer SSTs de entrada → merge ordenado → dropear versiones viejas / tombstones seguros → escribir SSTs nuevas en el nivel destino → editar MANIFEST → borrar inputs.

### Leveled (Jade / LevelDB / RocksDB default)

- L0: overlap permitido; trigger por **número de archivos**.  
- L≥1: archivos **no se solapan** por rango de user key; tamaño objetivo ~ `base * ratio^(level-1)`.  
- Un dato puede reescribirse ~ratio veces por nivel al bajar → **WA alta**, **RA baja**.

### Size-tiered / universal (no implementado como default en Jade)

Agrupa por tamaño similar. Menos WA, más archivos a tocar en lectura, más SA.

### Stalls vs throughput

Si la memtable se llena más rápido de lo que avanza flush+compaction → stalls. Medir `Db::stats().stall_count` es parte del narrative de portfolio.

---

## 12. Snapshots / MVCC (stretch)

`Db::snapshot()` fija `seq = next_seq` actual.

`get_snapshot(&snap, key)` usa `max_seq = snap.seq` → no ve writes posteriores.

El `SnapshotTracker` guarda seqs activos; `watermark()` = mínimo activo (o `u64::MAX` si no hay ninguno). Compaction usa el watermark para no GC’ear versiones que un snapshot aún podría necesitar (simplificado en Jade).

---

## 13. Mapa mental → archivos

```text
API pública          lib.rs, db.rs
Clave/versión        key.rs
Memoria              memtable.rs
Durabilidad volatile wal.rs
Disco inmutable      sstable/*
Metadatos            manifest.rs, version.rs
Mantenimiento        compact.rs, db.rs::{flush_imms,maybe_compact,bg_loop}
Lectura rango        iter.rs
MVCC                 snapshot.rs
Knobs                options.rs
```

---

## 14. Checklist de entrevista (con Jade)

Podés dibujar en pizarra:

1. LSM vs B-tree y las 3 amplificaciones.  
2. Invariante WAL → mem → ack; flush SST → manifest → drop WAL.  
3. Por qué L0 es especial; por qué bloom no puede dar falsos negativos.  
4. Leveled vs tiered; cuándo aparecen stalls.  
5. Tombstone + seq; riesgo de resurrección.  
6. Un número tuyo: tabla fsync Always/Batch/Never (`jade bench`).

---

## 15. Lecturas recomendadas

1. Esta guía + tu crash course mental de amplificaciones.  
2. [Mini-LSM](https://skyzh.github.io/mini-lsm/) — curso paralelo en Rust.  
3. [LevelDB Implementation](https://github.com/google/leveldb/blob/main/doc/impl.md) — corto y canónico.  
4. [RocksDB Compaction](https://github.com/facebook/rocksdb/wiki/Compaction) / Universal.  
5. Paper original: O’Neil et al., *The Log-Structured Merge-Tree* (1996).  
6. Grafo del repo: `graphify-out/GRAPH_REPORT.md` (hubs: `InternalKey`, `Db`, `SsTable`, …).

---

## 16. Cómo experimentar

```bash
cargo test
cargo run --bin jade -- bench always
cargo run --bin jade -- bench batch
cargo run --bin jade -- bench never
cargo bench
```

Tras cambiar código:

```bash
graphify update .
```

Abrí `graphify-out/graph.html` y filtrá por `Db`, `Wal`, `flush_imms`, `pick_compaction`.
