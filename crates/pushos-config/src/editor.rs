//! Editing configuration without destroying it.
//!
//! Studio is not the source of truth; these files are, and a person wrote them.
//! Edits are therefore applied to the parsed document rather than by
//! re-serialising a model, so comments, ordering and layout survive. A file
//! that has been saved by Studio should still be a file its author recognises.
//!
//! Nothing is written until the whole configuration has been validated. An edit
//! that would produce an unusable surface changes nothing on disk.

use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Table, Value, value};
use tracing::debug;

use crate::build::RuntimeConfig;
use crate::error::ConfigError;
use crate::model::ConfigFile;
use crate::paths;
use crate::spec::{BindingAddress, BindingSpec};

/// The array of tables bindings live in.
const BINDINGS: &str = "bindings";

/// Every configuration file under a root, parsed but not interpreted.
#[derive(Debug)]
pub struct ConfigDocuments {
    root: PathBuf,
    files: Vec<ConfigDocument>,
}

/// One parsed configuration file.
#[derive(Debug)]
struct ConfigDocument {
    path: PathBuf,
    document: DocumentMut,
    dirty: bool,
}

/// What an edit did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingEdit {
    /// The file that changed.
    pub file: PathBuf,
    /// Whether an existing binding was replaced rather than one added.
    pub replaced: bool,
}

impl ConfigDocuments {
    /// Reads every configuration file under `root`.
    ///
    /// A root with no files yet is not an error: the first edit creates the
    /// main file.
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let root = root.into();
        let mut files = Vec::new();

        for path in paths::files_for(&root) {
            let text =
                std::fs::read_to_string(&path).map_err(|source| ConfigError::Unreadable {
                    path: path.clone(),
                    source,
                })?;
            let document = text.parse::<DocumentMut>().map_err(|source| {
                // `toml_edit` and `toml` report the same class of problem; the
                // caller only needs to know which file and why.
                ConfigError::Unreadable {
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        source.to_string(),
                    ),
                }
            })?;
            files.push(ConfigDocument {
                path,
                document,
                dirty: false,
            });
        }

        Ok(Self { root, files })
    }

    /// The files that were read, in merge order.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.path.as_path())
    }

    /// Whether any file has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.files.iter().any(|file| file.dirty)
    }

    /// Adds a binding, or replaces the one already at that address.
    ///
    /// An existing binding is edited in the file it was written in, so a
    /// configuration split across several files stays organised the way its
    /// author organised it. A new binding goes in the main file.
    pub fn upsert_binding(&mut self, spec: &BindingSpec) -> BindingEdit {
        if let Some(index) = self.locate(&spec.address) {
            let file = &mut self.files[index.file];
            if let Some(table) =
                binding_tables_mut(&mut file.document).and_then(|a| a.get_mut(index.entry))
            {
                write_binding(table, spec);
                file.dirty = true;
                return BindingEdit {
                    file: file.path.clone(),
                    replaced: true,
                };
            }
        }

        let index = self.main_file_index();
        let file = &mut self.files[index];
        let mut table = Table::new();
        table.set_implicit(false);
        write_binding(&mut table, spec);

        if let Some(bindings) = binding_tables_mut(&mut file.document) {
            bindings.push(table);
            file.dirty = true;
        }

        BindingEdit {
            file: file.path.clone(),
            replaced: false,
        }
    }

    /// Removes the binding at an address.
    ///
    /// Returns the file it was removed from, or `None` when nothing was there.
    pub fn remove_binding(&mut self, address: &BindingAddress) -> Option<PathBuf> {
        let index = self.locate(address)?;
        let file = &mut self.files[index.file];
        binding_tables_mut(&mut file.document)?.remove(index.entry);
        file.dirty = true;
        Some(file.path.clone())
    }

    /// Validates the edited configuration and writes what changed.
    ///
    /// Nothing reaches the disk unless the whole configuration is usable, and
    /// each file is replaced atomically, so a reader never sees half a file.
    pub fn save(&mut self) -> Result<(), ConfigError> {
        self.validate()?;

        for file in self.files.iter_mut().filter(|file| file.dirty) {
            write_atomically(&file.path, &file.document.to_string())?;
            debug!(path = ?file.path, "configuration written");
            file.dirty = false;
        }
        Ok(())
    }

    /// Checks that the edited configuration would build, without writing.
    pub fn validate(&self) -> Result<RuntimeConfig, ConfigError> {
        let mut merged = ConfigFile::default();
        for file in &self.files {
            let parsed: ConfigFile =
                toml::from_str(&file.document.to_string()).map_err(|source| {
                    ConfigError::Malformed {
                        path: file.path.clone(),
                        source,
                    }
                })?;
            merged.merge(parsed);
        }
        RuntimeConfig::build(&merged)
    }

    /// Finds where a binding lives.
    fn locate(&self, address: &BindingAddress) -> Option<Located> {
        self.files.iter().enumerate().find_map(|(file, document)| {
            binding_tables(&document.document)?
                .iter()
                .position(|entry| entry_address(entry).as_ref() == Some(address))
                .map(|entry| Located { file, entry })
        })
    }

    /// The file new bindings are added to, creating it if the root is empty.
    fn main_file_index(&mut self) -> usize {
        let main = if self.root.extension().is_some() {
            self.root.clone()
        } else {
            self.root.join(paths::MAIN_FILE)
        };

        if let Some(index) = self.files.iter().position(|file| file.path == main) {
            ensure_bindings_array(&mut self.files[index].document);
            return index;
        }

        let mut document = DocumentMut::new();
        ensure_bindings_array(&mut document);
        self.files.push(ConfigDocument {
            path: main,
            document,
            dirty: true,
        });
        self.files.len() - 1
    }
}

/// Where a binding was found.
struct Located {
    file: usize,
    entry: usize,
}

fn binding_tables(document: &DocumentMut) -> Option<&toml_edit::ArrayOfTables> {
    document.get(BINDINGS)?.as_array_of_tables()
}

fn binding_tables_mut(document: &mut DocumentMut) -> Option<&mut toml_edit::ArrayOfTables> {
    ensure_bindings_array(document);
    document.get_mut(BINDINGS)?.as_array_of_tables_mut()
}

fn ensure_bindings_array(document: &mut DocumentMut) {
    if document
        .get(BINDINGS)
        .and_then(Item::as_array_of_tables)
        .is_none()
    {
        document[BINDINGS] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
}

/// Reads the address out of an existing entry.
fn entry_address(entry: &Table) -> Option<BindingAddress> {
    Some(BindingAddress {
        control: entry.get("control")?.as_str()?.to_owned(),
        gesture: entry.get("gesture")?.as_str()?.to_owned(),
        page: entry
            .get("page")
            .and_then(Item::as_str)
            .map(ToOwned::to_owned),
        workspace: entry
            .get("workspace")
            .and_then(Item::as_str)
            .map(ToOwned::to_owned),
    })
}

/// Writes a specification into a table, leaving unrelated keys alone.
fn write_binding(table: &mut Table, spec: &BindingSpec) {
    table["control"] = value(spec.address.control.as_str());
    table["gesture"] = value(spec.address.gesture.as_str());
    table["action"] = value(spec.action.as_str());

    set_or_remove(table, "page", spec.address.page.as_deref());
    set_or_remove(table, "workspace", spec.address.workspace.as_deref());
    set_or_remove(table, "target", spec.target.as_deref());
    set_or_remove(table, "label", spec.label.as_deref());

    if spec.priority == 0 {
        table.remove("priority");
    } else {
        table["priority"] = value(i64::from(spec.priority));
    }

    if spec.params.is_empty() {
        table.remove("params");
    } else {
        let mut params = toml_edit::InlineTable::new();
        for (key, param) in &spec.params {
            params.insert(key, param_value(param));
        }
        table["params"] = Item::Value(Value::InlineTable(params));
    }
}

fn set_or_remove(table: &mut Table, key: &str, content: Option<&str>) {
    match content {
        Some(content) => table[key] = value(content),
        None => {
            table.remove(key);
        }
    }
}

fn param_value(param: &pushos_domain::action::ParamValue) -> Value {
    use pushos_domain::action::ParamValue as P;
    match param {
        P::Text(text) => Value::from(text.as_ref()),
        P::Integer(number) => Value::from(*number),
        P::Number(number) => Value::from(*number),
        P::Flag(flag) => Value::from(*flag),
        P::List(items) => Value::Array(items.iter().map(param_value).collect::<Array>()),
    }
}

/// Replaces a file's contents in one step.
///
/// Writing in place would let a reader, including PushOS's own watcher, observe
/// a half-written file. A temporary file in the same directory followed by a
/// rename cannot be observed partially.
fn write_atomically(path: &Path, contents: &str) -> Result<(), ConfigError> {
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory).map_err(|source| ConfigError::Unwritable {
        path: directory.to_path_buf(),
        source,
    })?;

    let temporary = path.with_extension("toml.pushos-tmp");
    std::fs::write(&temporary, contents).map_err(|source| ConfigError::Unwritable {
        path: temporary.clone(),
        source,
    })?;
    std::fs::rename(&temporary, path).map_err(|source| ConfigError::Unwritable {
        path: path.to_path_buf(),
        source,
    })
}
