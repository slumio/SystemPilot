use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CredentialRef {
    #[default]
    None,
    Environment {
        variable: String,
    },
    File {
        path: PathBuf,
    },
    Systemd {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredentialStatus {
    pub source: &'static str,
    pub available: bool,
}

pub struct CredentialResolver;

impl CredentialResolver {
    pub fn resolve(reference: &CredentialRef) -> AppResult<Option<String>> {
        let value = match reference {
            CredentialRef::None => return Ok(None),
            CredentialRef::Environment { variable } => std::env::var(variable).map_err(|_| {
                AppError::Validation(format!(
                    "credential environment variable {variable} is unavailable"
                ))
            })?,
            CredentialRef::File { path } => read_owner_file(path)?,
            CredentialRef::Systemd { name } => {
                validate_name(name)?;
                let directory = std::env::var_os("CREDENTIALS_DIRECTORY").ok_or_else(|| {
                    AppError::Validation("systemd CREDENTIALS_DIRECTORY is unavailable".into())
                })?;
                read_owner_file(&PathBuf::from(directory).join(name))?
            }
        };
        let value = value.trim_end_matches(['\r', '\n']).to_string();
        if value.is_empty() {
            return Err(AppError::Validation("credential source is empty".into()));
        }
        Ok(Some(value))
    }

    pub fn status(reference: &CredentialRef) -> CredentialStatus {
        CredentialStatus {
            source: match reference {
                CredentialRef::None => "none",
                CredentialRef::Environment { .. } => "environment",
                CredentialRef::File { .. } => "file",
                CredentialRef::Systemd { .. } => "systemd",
            },
            available: Self::resolve(reference).is_ok_and(|value| value.is_some()),
        }
    }
}

fn validate_name(name: &str) -> AppResult<()> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err(AppError::Validation(
            "invalid systemd credential name".into(),
        ));
    }
    Ok(())
}

fn read_owner_file(path: &Path) -> AppResult<String> {
    let metadata =
        fs::metadata(path).map_err(|error| AppError::io("read credential metadata", error))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(AppError::Validation(format!(
            "credential file {} must not be accessible by group or others",
            path.display()
        )));
    }
    fs::read_to_string(path).map_err(|error| AppError::io("read credential file", error))
}

pub fn store_owner_secret(directory: &Path, name: &str, value: &str) -> AppResult<CredentialRef> {
    validate_name(name)?;
    if value.trim().is_empty() {
        return Err(AppError::Validation("credential must not be empty".into()));
    }
    fs::create_dir_all(directory)
        .map_err(|error| AppError::io("create credential directory", error))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| AppError::io("secure credential directory", error))?;
    let path = directory.join(name);
    let temporary = directory.join(format!(".{name}-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| AppError::io("create temporary credential", error))?;
    if let Err(error) = file
        .write_all(value.as_bytes())
        .and_then(|_| file.sync_all())
    {
        if let Err(cleanup) = fs::remove_file(&temporary) {
            eprintln!("credential temporary-file cleanup failed: {cleanup}");
        }
        return Err(AppError::io("persist credential", error));
    }
    fs::rename(&temporary, &path).map_err(|error| AppError::io("commit credential", error))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| AppError::io("secure credential", error))?;
    Ok(CredentialRef::File { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_file_round_trip_and_permissions() {
        let directory = tempfile::tempdir().unwrap().path().join("credentials");
        let reference = store_owner_secret(&directory, "token", "secret").unwrap();
        assert_eq!(
            CredentialResolver::resolve(&reference).unwrap().as_deref(),
            Some("secret")
        );
        let CredentialRef::File { path } = reference else {
            panic!("expected file reference")
        };
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn permissive_and_empty_credentials_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("token");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(CredentialResolver::resolve(&CredentialRef::File { path }).is_err());
        assert!(store_owner_secret(directory.path(), "empty", "").is_err());
    }
}
