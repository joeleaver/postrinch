//! The install folder on the person's own PATH (`HKCU\Environment`, value
//! `Path`), so a terminal runs `postrinch-cli` by name, as the Linux links
//! from `~/.local/bin` let it. The entry is added once, and an entry the
//! person had before stays theirs: the uninstall removes the entry only
//! when Postrinch's key (`STATE_KEY`, value [`PATH_ADDED`]) says the
//! install added it. The Setup keeps the same record in its `[Code]`.

use std::path::Path;

use windows_registry::{Key, Type};

use super::{STATE_KEY, expand, not_found};
use crate::path_list;

const ENVIRONMENT: &str = "Environment";
const PATH: &str = "Path";
/// The entry the install added, as it wrote it.
pub(crate) const PATH_ADDED: &str = "PathAdded";

/// How the folder came to be on the PATH.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Added {
    /// By this install, just now.
    Now,
    /// By an earlier install (the Setup's or `--install`'s).
    Before,
    /// By the person, so it is never removed.
    ByThePerson,
}

/// Put `dir` on the PATH in `user`'s environment unless it is there.
pub(crate) fn add(user: &Key, dir: &Path) -> Result<Added, String> {
    let dir = dir.display().to_string();
    let env = user
        .create(ENVIRONMENT)
        .map_err(|e| format!("opening HKCU\\{ENVIRONMENT}: {e}"))?;
    let (path, ty) = read(&env)?;
    if path_list::has(&path, &dir, &expand) {
        return Ok(if recorded(user)?.is_some() {
            Added::Before
        } else {
            Added::ByThePerson
        });
    }
    write(&env, &path_list::with(&path, &dir), ty)?;
    user.create(STATE_KEY)
        .and_then(|state| state.set_string(PATH_ADDED, &dir))
        .map_err(|e| format!("recording the PATH entry in HKCU\\{STATE_KEY}: {e}"))?;
    Ok(Added::Now)
}

/// Take `dir` off the PATH if the install put it there.
pub(crate) fn remove(user: &Key, dir: &Path) -> Result<(), String> {
    let Some(added) = recorded(user)? else {
        return Ok(());
    };
    let env = match user.options().read().write().open(ENVIRONMENT) {
        Ok(env) => env,
        Err(e) if not_found(&e) => return Ok(()),
        Err(e) => return Err(format!("opening HKCU\\{ENVIRONMENT}: {e}")),
    };
    let (path, ty) = read(&env)?;
    let mut left = path_list::without(&path, &added, &expand);
    let dir = dir.display().to_string();
    if dir != added {
        left = path_list::without(&left, &dir, &expand);
    }
    if left != path {
        write(&env, &left, ty)?;
    }
    Ok(())
}

/// The entry the install recorded adding, if it did.
fn recorded(user: &Key) -> Result<Option<String>, String> {
    let state = match user.open(STATE_KEY) {
        Ok(state) => state,
        Err(e) if not_found(&e) => return Ok(None),
        Err(e) => return Err(format!("opening HKCU\\{STATE_KEY}: {e}")),
    };
    match state.get_string(PATH_ADDED) {
        Ok(added) => Ok(Some(added)),
        Err(e) if not_found(&e) => Ok(None),
        Err(e) => Err(format!("reading HKCU\\{STATE_KEY}\\{PATH_ADDED}: {e}")),
    }
}

/// The person's PATH and how it is stored. A new one is stored expandable,
/// as Windows' own editor stores it.
fn read(env: &Key) -> Result<(String, Type), String> {
    let ty = match env.get_type(PATH) {
        Ok(ty) => ty,
        Err(e) if not_found(&e) => return Ok((String::new(), Type::ExpandString)),
        Err(e) => return Err(format!("reading your PATH: {e}")),
    };
    if !matches!(ty, Type::String | Type::ExpandString) {
        return Err(format!(
            "your PATH (HKCU\\{ENVIRONMENT}\\{PATH}) is not text, so it was left alone"
        ));
    }
    // As written: an expandable value's `%VAR%`s stay as they are, and are
    // written back so.
    let path = env
        .get_hstring(PATH)
        .map_err(|e| format!("reading your PATH: {e}"))?
        .to_string_lossy();
    Ok((path, ty))
}

fn write(env: &Key, path: &str, ty: Type) -> Result<(), String> {
    let written = if ty == Type::String {
        env.set_string(PATH, path)
    } else {
        env.set_expand_string(PATH, path)
    };
    written.map_err(|e| format!("writing your PATH: {e}"))
}
