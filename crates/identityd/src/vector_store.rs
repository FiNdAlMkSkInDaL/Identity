use arrow_array::types::Float32Type;
use arrow_array::{Array, BinaryArray, FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use crate::workspace::IdentityPaths;
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{Connection as LanceConnection, Error as LanceError, Table as LanceTable};
use rusqlite::Connection;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::runtime::Builder as TokioRuntimeBuilder;

const LANCEDB_DIR: &str = "lancedb";
const LANCEDB_TABLE: &str = "memory_vectors";
const STORE_META_FILE: &str = "store.meta";
const VECTOR_FILE_SUFFIX: &str = ".f32le";
const FORMAT_VERSION: &str = "raw-f32-le-v1";

#[derive(Debug)]
pub enum VectorStoreError {
    Io(io::Error),
    Arrow(ArrowError),
    InvalidData(String),
    LanceDb(LanceError),
    Runtime(String),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for VectorStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Arrow(error) => write!(f, "{error}"),
            Self::InvalidData(message) => write!(f, "{message}"),
            Self::LanceDb(error) => write!(f, "{error}"),
            Self::Runtime(message) => write!(f, "{message}"),
            Self::Sqlite(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for VectorStoreError {}

impl From<io::Error> for VectorStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ArrowError> for VectorStoreError {
    fn from(value: ArrowError) -> Self {
        Self::Arrow(value)
    }
}

impl From<LanceError> for VectorStoreError {
    fn from(value: LanceError) -> Self {
        Self::LanceDb(value)
    }
}

impl From<rusqlite::Error> for VectorStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

pub trait VectorBlobStore {
    fn backend_name(&self) -> &'static str;
    fn upsert(&self, node_id: i64, bytes: &[u8]) -> Result<(), VectorStoreError>;
    fn read(&self, node_id: i64) -> Result<Option<Vec<u8>>, VectorStoreError>;
}

pub struct VectorStore {
    primary: Box<dyn VectorBlobStore>,
    mirror: Box<dyn VectorBlobStore>,
    fallback: Box<dyn VectorBlobStore>,
}

#[derive(Debug, Clone)]
struct LanceDbVectorStore {
    schema: SchemaRef,
    table: LanceTable,
    dimension: usize,
}

#[derive(Debug, Clone)]
struct FilesystemVectorStore {
    root: PathBuf,
    model_id: String,
    dimension: usize,
}

#[derive(Debug, Clone)]
struct SqliteVectorStore {
    identity_db: PathBuf,
}

impl VectorStore {
    pub fn open(
        paths: &IdentityPaths,
        model_id: &str,
        dimension: usize,
    ) -> Result<Self, VectorStoreError> {
        let lance = LanceDbVectorStore::open(paths.vector_store_dir.join(LANCEDB_DIR), dimension)?;
        let filesystem = FilesystemVectorStore {
            root: paths.vector_store_dir.clone(),
            model_id: model_id.to_string(),
            dimension,
        };
        filesystem.ensure_layout()?;
        let fallback = SqliteVectorStore {
            identity_db: paths.identity_db.clone(),
        };
        Ok(Self {
            primary: Box::new(lance),
            mirror: Box::new(filesystem),
            fallback: Box::new(fallback),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        "lancedb+filesystem+sqlite"
    }

    pub fn upsert(&self, node_id: i64, bytes: &[u8]) -> Result<(), VectorStoreError> {
        self.primary.upsert(node_id, bytes)?;
        self.mirror.upsert(node_id, bytes)
    }

    pub fn read(&self, node_id: i64) -> Result<Option<Vec<u8>>, VectorStoreError> {
        if let Some(bytes) = self.primary.read(node_id)? {
            Ok(Some(bytes))
        } else if let Some(bytes) = self.mirror.read(node_id)? {
            Ok(Some(bytes))
        } else {
            self.fallback.read(node_id)
        }
    }
}

impl LanceDbVectorStore {
    fn open(root: PathBuf, dimension: usize) -> Result<Self, VectorStoreError> {
        fs::create_dir_all(&root)?;

        let schema = vector_table_schema(dimension);
        let root_string = root.to_string_lossy().into_owned();
        let table_schema = schema.clone();
        let table = run_lancedb(async move {
            let db = lancedb::connect(&root_string).execute().await?;
            ensure_table(&db, table_schema).await
        })?;

        Ok(Self {
            schema,
            table,
            dimension,
        })
    }

    fn record_batch(&self, node_id: i64, bytes: &[u8]) -> Result<RecordBatch, VectorStoreError> {
        let vector = decode_f32_bytes(bytes, self.dimension)?;
        let id_column = Arc::new(Int64Array::from(vec![node_id]));
        let vector_column = Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            std::iter::once(Some(vector.into_iter().map(Some).collect::<Vec<_>>())),
            self.dimension as i32,
        ));
        let blob_column = Arc::new(BinaryArray::from_iter_values(std::iter::once(bytes)));

        Ok(RecordBatch::try_new(
            self.schema.clone(),
            vec![id_column, vector_column, blob_column],
        )?)
    }
}

impl VectorBlobStore for LanceDbVectorStore {
    fn backend_name(&self) -> &'static str {
        "lancedb"
    }

    fn upsert(&self, node_id: i64, bytes: &[u8]) -> Result<(), VectorStoreError> {
        let batch = self.record_batch(node_id, bytes)?;
        let schema = self.schema.clone();
        let table = self.table.clone();

        run_lancedb(async move {
            let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
            let mut merge = table.merge_insert(&["id"]);
            merge.when_matched_update_all(None).when_not_matched_insert_all();
            merge.execute(Box::new(reader)).await?;
            Ok(())
        })
    }

    fn read(&self, node_id: i64) -> Result<Option<Vec<u8>>, VectorStoreError> {
        let table = self.table.clone();

        let batches = run_lancedb(async move {
            let batches = table
                .query()
                .only_if(format!("id = {node_id}"))
                .limit(1)
                .execute()
                .await?
                .try_collect::<Vec<RecordBatch>>()
                .await?;

            Ok::<Vec<RecordBatch>, LanceError>(batches)
        })?;

        extract_blob_bytes(&batches)
    }
}

impl FilesystemVectorStore {
    fn ensure_layout(&self) -> Result<(), VectorStoreError> {
        fs::create_dir_all(&self.root)?;
        fs::write(self.metadata_path(), self.metadata_contents())?;
        Ok(())
    }

    fn metadata_contents(&self) -> String {
        format!(
            "format={FORMAT_VERSION}\nmodel_id={}\nembedding_dim={}\nblob_len={}\n",
            self.model_id,
            self.dimension,
            self.dimension * std::mem::size_of::<f32>()
        )
    }

    fn metadata_path(&self) -> PathBuf {
        self.root.join(STORE_META_FILE)
    }

    fn vector_path(&self, node_id: i64) -> PathBuf {
        self.root.join(format!("node-{node_id:020}{VECTOR_FILE_SUFFIX}"))
    }
}

impl VectorBlobStore for FilesystemVectorStore {
    fn backend_name(&self) -> &'static str {
        "filesystem"
    }

    fn upsert(&self, node_id: i64, bytes: &[u8]) -> Result<(), VectorStoreError> {
        let target = self.vector_path(node_id);
        let temp = target.with_extension("tmp");
        fs::write(&temp, bytes)?;
        fs::rename(temp, target)?;
        Ok(())
    }

    fn read(&self, node_id: i64) -> Result<Option<Vec<u8>>, VectorStoreError> {
        let path = self.vector_path(node_id);

        if !path.exists() {
            return Ok(None);
        }

        Ok(Some(fs::read(path)?))
    }
}

impl VectorBlobStore for SqliteVectorStore {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    fn upsert(&self, _node_id: i64, _bytes: &[u8]) -> Result<(), VectorStoreError> {
        Ok(())
    }

    fn read(&self, node_id: i64) -> Result<Option<Vec<u8>>, VectorStoreError> {
        let conn = Connection::open(&self.identity_db)?;
        let mut statement = conn.prepare(
            "SELECT vector_embedding
             FROM memory_nodes
             WHERE id = ?1",
        )?;
        let mut rows = statement.query([node_id])?;

        if let Some(row) = rows.next()? {
            let blob: Option<Vec<u8>> = row.get(0)?;
            Ok(blob)
        } else {
            Ok(None)
        }
    }
}

fn vector_table_schema(dimension: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension as i32,
            ),
            false,
        ),
        Field::new("blob", DataType::Binary, false),
    ]))
}

async fn ensure_table(
    db: &LanceConnection,
    schema: SchemaRef,
) -> Result<LanceTable, LanceError> {
    match db.open_table(LANCEDB_TABLE).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            db.create_empty_table(LANCEDB_TABLE, schema).execute().await?;
            db.open_table(LANCEDB_TABLE).execute().await
        }
    }
}

fn run_lancedb<F, T>(future: F) -> Result<T, VectorStoreError>
where
    F: std::future::Future<Output = Result<T, LanceError>> + Send + 'static,
    T: Send + 'static,
{
    let task = thread::spawn(move || -> Result<T, VectorStoreError> {
        let runtime = TokioRuntimeBuilder::new_current_thread().enable_all().build()?;
        runtime.block_on(future).map_err(VectorStoreError::from)
    });

    task.join().map_err(|_| {
        VectorStoreError::Runtime("lancedb runtime thread panicked".to_string())
    })?
}

fn decode_f32_bytes(bytes: &[u8], dimension: usize) -> Result<Vec<f32>, VectorStoreError> {
    let expected_len = dimension * std::mem::size_of::<f32>();
    if bytes.len() != expected_len {
        return Err(VectorStoreError::InvalidData(format!(
            "expected {expected_len} vector bytes, got {}",
            bytes.len()
        )));
    }

    Ok(bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let mut raw = [0_u8; std::mem::size_of::<f32>()];
            raw.copy_from_slice(chunk);
            f32::from_le_bytes(raw)
        })
        .collect())
}

fn extract_blob_bytes(batches: &[RecordBatch]) -> Result<Option<Vec<u8>>, VectorStoreError> {
    let Some(batch) = batches.first() else {
        return Ok(None);
    };

    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let Some(column) = batch.column_by_name("blob") else {
        return Err(VectorStoreError::InvalidData(
            "lancedb vector table is missing blob column".to_string(),
        ));
    };
    let Some(binary) = column.as_any().downcast_ref::<BinaryArray>() else {
        return Err(VectorStoreError::InvalidData(
            "lancedb blob column has unexpected type".to_string(),
        ));
    };

    if binary.is_null(0) {
        Ok(None)
    } else {
        Ok(Some(binary.value(0).to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::VectorStore;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn vector_store_creates_metadata_and_round_trips_vectors() {
        let root = std::env::temp_dir().join(format!(
            "identity-vector-store-module-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = VectorStore::open(&paths, "test-model", 4).unwrap();
        let test_data = [1_u8; 16];
        store.upsert(12, &test_data).unwrap();

        let meta = fs::read_to_string(paths.vector_store_dir.join("store.meta")).unwrap();
        let vector = store.read(12).unwrap().unwrap();

        assert!(meta.contains("format=raw-f32-le-v1"));
        assert!(meta.contains("model_id=test-model"));
        assert!(meta.contains("embedding_dim=4"));
        assert!(paths.vector_store_dir.join("lancedb").exists());
        assert_eq!(store.backend_name(), "lancedb+filesystem+sqlite");
        assert_eq!(vector, test_data.to_vec());

        fs::remove_dir_all(root).unwrap();
    }
}