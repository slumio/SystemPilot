//! Atomic, versioned configuration migration and rollback.
use crate::credentials::{store_owner_secret, CredentialRef};
use crate::error::{AppError, AppResult};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const CURRENT_CONFIG_SCHEMA_VERSION: u64 = 2;
const PRE_V1_BACKUP: &str = "config.pre-v1.json";
const PRE_V2_BACKUP: &str = "config.pre-v2.json";
const PRE_ROLLBACK_BACKUP: &str = "config.pre-rollback-v2.json";

fn secure_parent(path: &Path) -> AppResult<&Path> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Validation("configuration path has no parent".into()))?;
    fs::create_dir_all(parent)
        .map_err(|e| AppError::io("could not create configuration directory", e))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|e| AppError::io("could not secure configuration directory", e))?;
    Ok(parent)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = secure_parent(path)?;
    let temporary = parent.join(format!(".config-migration-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| AppError::io("could not create temporary migrated configuration", e))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        if let Err(cleanup) = fs::remove_file(&temporary) {
            eprintln!("configuration temporary-file cleanup failed: {cleanup}");
        }
        return Err(AppError::io(
            "could not persist migrated configuration",
            error,
        ));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if let Err(cleanup) = fs::remove_file(&temporary) {
            eprintln!("configuration temporary-file cleanup failed: {cleanup}");
        }
        return Err(AppError::io(
            "could not commit migrated configuration",
            error,
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| AppError::io("could not secure migrated configuration", e))?;
    let directory = fs::File::open(parent)
        .map_err(|e| AppError::io("could not open configuration directory for sync", e))?;
    directory
        .sync_all()
        .map_err(|e| AppError::io("could not sync configuration directory", e))
}

fn immutable_backup(path: &Path, bytes: &[u8]) -> AppResult<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| AppError::io("could not persist configuration backup", e)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(path)
                .map_err(|e| AppError::io("could not read existing configuration backup", e))?;
            if existing == bytes {
                Ok(())
            } else {
                Err(AppError::Validation(format!("configuration backup {} already exists with different contents; refusing to overwrite it", path.display())))
            }
        }
        Err(error) => Err(AppError::io("could not create configuration backup", error)),
    }
}

pub fn load_and_migrate(path: &Path) -> AppResult<String> {
    let original =
        fs::read(path).map_err(|e| AppError::io("could not read SysPilot configuration", e))?;
    let mut value: Value =
        serde_json::from_slice(&original).map_err(|source| AppError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("configuration root must be a JSON object".into()))?;
    let version = match object.get("schema_version") {
        None => 0,
        Some(value) => value.as_u64().ok_or_else(|| {
            AppError::Validation("configuration schema_version must be an unsigned integer".into())
        })?,
    };
    match version {
        CURRENT_CONFIG_SCHEMA_VERSION => String::from_utf8(original).map_err(|e| AppError::Validation(format!("configuration is not UTF-8: {e}"))),
        0 | 1 => {
            let parent = secure_parent(path)?;
            if version == 0 {
                immutable_backup(&parent.join(PRE_V1_BACKUP), &original)?;
            }
            immutable_backup(&parent.join(PRE_V2_BACKUP), &original)?;
            if let Some(model) = object.get("gemini_model").and_then(Value::as_str) {
                if model.is_empty() || model == "gemini" || model == "gemini-2.0-flash" || (!model.contains('/') && !model.contains('-')) {
                    object.insert("gemini_model".into(), Value::from("gemini-3.6-flash"));
                }
            }
            let credential_dir = parent.join("credentials");
            migrate_secret(
                object,
                "gemini_api_key",
                "gemini_credential",
                &credential_dir,
                "gemini-api-key",
            )?;
            migrate_secret(
                object,
                "syspilot_api_key",
                "syspilot_credential",
                &credential_dir,
                "syspilot-api-key",
            )?;
            if let Some(telemetry) = object
                .get_mut("distributed_telemetry")
                .and_then(Value::as_object_mut)
            {
                migrate_secret(
                    telemetry,
                    "bearer_token",
                    "bearer_credential",
                    &credential_dir,
                    "telemetry-token",
                )?;
            }
            if let Some(fleet) = object.get_mut("fleet").and_then(Value::as_object_mut) {
                migrate_secret(
                    fleet,
                    "credential",
                    "credential_ref",
                    &credential_dir,
                    "fleet-token",
                )?;
            }
            object.insert("schema_version".into(), Value::from(CURRENT_CONFIG_SCHEMA_VERSION));
            let migrated = serde_json::to_vec_pretty(&value).map_err(|e| AppError::Protocol(format!("could not encode migrated configuration: {e}")))?;
            atomic_write(path, &migrated)?;
            String::from_utf8(migrated).map_err(|e| AppError::Validation(format!("migrated configuration is not UTF-8: {e}")))
        }
        newer => Err(AppError::Validation(format!("configuration schema {newer} is newer than supported schema {CURRENT_CONFIG_SCHEMA_VERSION}; upgrade SysPilot or run `syspilot config rollback`"))),
    }
}

fn migrate_secret(
    object: &mut serde_json::Map<String, Value>,
    old_key: &str,
    reference_key: &str,
    directory: &Path,
    filename: &str,
) -> AppResult<()> {
    let Some(value) = object.remove(old_key) else {
        return Ok(());
    };
    let secret = value.as_str().ok_or_else(|| {
        AppError::Validation(format!(
            "legacy credential field {old_key} must be a string"
        ))
    })?;
    if secret.is_empty() {
        object.insert(
            reference_key.into(),
            serde_json::to_value(CredentialRef::None)
                .map_err(|error| AppError::Protocol(error.to_string()))?,
        );
        return Ok(());
    }
    let reference = store_owner_secret(directory, filename, secret)?;
    object.insert(
        reference_key.into(),
        serde_json::to_value(reference).map_err(|error| AppError::Protocol(error.to_string()))?,
    );
    Ok(())
}

pub fn rollback(path: &Path) -> AppResult<PathBuf> {
    let parent = secure_parent(path)?;
    let backup = if parent.join(PRE_V2_BACKUP).exists() {
        parent.join(PRE_V2_BACKUP)
    } else {
        parent.join(PRE_V1_BACKUP)
    };
    let previous = fs::read(&backup)
        .map_err(|e| AppError::io("could not read pre-migration configuration backup", e))?;
    if path.exists() {
        let current = fs::read(path)
            .map_err(|e| AppError::io("could not read current configuration before rollback", e))?;
        immutable_backup(&parent.join(PRE_ROLLBACK_BACKUP), &current)?;
    }
    atomic_write(path, &previous)?;
    Ok(backup)
}
