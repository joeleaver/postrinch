//! The install's place on the desktop. On Linux: links on the PATH, the icon
//! in the hicolor theme and a menu entry. On Windows: a Start-menu entry,
//! the folder on the PATH and an entry in Apps & features, the same as the
//! Setup leaves (`crate::win`). Elsewhere nothing yet.

use std::path::Path;

#[cfg(target_os = "linux")]
mod linux;

/// The icon, as the window crate carries it.
pub struct Icons {
    /// Square PNGs by edge length, for `hicolor/<n>x<n>/apps`.
    pub pngs: &'static [(u32, &'static [u8])],
    pub svg: &'static [u8],
}

/// Put the install in `dir` on the desktop. Answers what the person should
/// hear about it (something left alone, say), to print once the install is
/// done: `--install` runs before logging starts.
pub(crate) fn integrate(dir: &Path, icons: &Icons) -> Result<Vec<String>, String> {
    #[cfg(target_os = "linux")]
    return linux::integrate(dir, icons);
    #[cfg(windows)]
    {
        let _ = icons; // The icon is built into postrinch.exe.
        crate::win::integrate(&crate::win::Places::user()?, dir, crate::CURRENT)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (dir, icons);
        Ok(Vec::new())
    }
}

/// An update installed `version`: say so where the system shows the
/// installed version (Apps & features on Windows).
pub(crate) fn record_version(version: &str) -> Result<(), String> {
    #[cfg(windows)]
    return crate::win::registration::record_version(&crate::win::Places::user()?.user, version);
    #[cfg(not(windows))]
    {
        let _ = version;
        Ok(())
    }
}
