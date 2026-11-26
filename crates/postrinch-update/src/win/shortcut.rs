//! The Start-menu entry: a shell link (`.lnk`) to `postrinch.exe`, made
//! through the Shell's own `IShellLinkW`, with the icon built into the
//! program. Writing it again replaces it, so there is only ever one.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{Interface, PCWSTR};

use super::wide;

/// What the Start menu shows when the pointer rests on the entry; the
/// Setup's `[Icons]` says the same.
pub(crate) const DESCRIPTION: &str = "Your mail, on your computer, with agents you let in";

/// The person's Start menu › Programs folder.
pub(crate) fn programs_folder() -> Result<PathBuf, String> {
    // SAFETY: the answer is a string the Shell allocated, copied and then
    // freed here.
    unsafe {
        let found = SHGetKnownFolderPath(&FOLDERID_Programs, KNOWN_FOLDER_FLAG(0), None)
            .map_err(|e| format!("finding the Start menu: {e}"))?;
        let path = found.to_string();
        CoTaskMemFree(Some(found.0 as *const _));
        path.map(PathBuf::from)
            .map_err(|e| format!("reading the Start menu's folder: {e}"))
    }
}

/// Write the shell link `lnk` starting `target` in `dir`, with `target`'s
/// own icon.
pub(crate) fn write(lnk: &Path, target: &Path, dir: &Path) -> Result<(), String> {
    if let Some(folder) = lnk.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("creating {}: {e}", folder.display()))?;
    }
    let _com = Com::init()?;
    let failed = |e: windows::core::Error| format!("making {}: {e}", lnk.display());
    let (target_w, dir_w, description, lnk_w) =
        (wide(target), wide(dir), wide(DESCRIPTION), wide(lnk));
    // SAFETY: COM is initialized on this thread for as long as `_com`
    // lives, and every string passed is NUL-terminated and outlives its call.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(failed)?;
        link.SetPath(PCWSTR(target_w.as_ptr())).map_err(failed)?;
        link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()))
            .map_err(failed)?;
        link.SetIconLocation(PCWSTR(target_w.as_ptr()), 0)
            .map_err(failed)?;
        link.SetDescription(PCWSTR(description.as_ptr()))
            .map_err(failed)?;
        let file: IPersistFile = link.cast().map_err(failed)?;
        file.Save(PCWSTR(lnk_w.as_ptr()), true).map_err(failed)?;
    }
    Ok(())
}

/// What a shell link says: its target and its icon's file and index.
#[cfg(test)]
pub(crate) fn read(lnk: &Path) -> Result<(PathBuf, PathBuf, i32), String> {
    use windows::Win32::System::Com::STGM_READ;
    let _com = Com::init()?;
    let failed = |e: windows::core::Error| format!("reading {}: {e}", lnk.display());
    let lnk_w = wide(lnk);
    let text = |buf: &[u16]| {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        PathBuf::from(String::from_utf16_lossy(&buf[..end]))
    };
    // SAFETY: as in `write`; the buffers are written within their lengths.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(failed)?;
        let file: IPersistFile = link.cast().map_err(failed)?;
        file.Load(PCWSTR(lnk_w.as_ptr()), STGM_READ)
            .map_err(failed)?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)
            .map_err(failed)?;
        let (mut icon, mut index) = ([0u16; 1024], 0);
        link.GetIconLocation(&mut icon, &mut index)
            .map_err(failed)?;
        Ok((text(&target), text(&icon), index))
    }
}

/// COM on this thread while it lives. A thread already in another
/// apartment keeps it: the shell link works in either.
struct Com {
    owned: bool,
}

impl Com {
    fn init() -> Result<Com, String> {
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        // SAFETY: paired with `CoUninitialize` in `drop` when it succeeds.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Com { owned: false });
        }
        hr.ok().map_err(|e| format!("starting COM: {e}"))?;
        Ok(Com { owned: true })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: this thread's own successful `CoInitializeEx`.
            unsafe { CoUninitialize() };
        }
    }
}
