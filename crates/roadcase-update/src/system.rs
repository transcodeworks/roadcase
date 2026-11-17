//! How the application was installed: by a system package manager, or not.
//!
//! The `.deb` and pacman packages roadcase makes install the application
//! under `/opt/<package>/`, with its programs in `/opt/<package>/bin/`, and a
//! marker file, [`INSTALL_MARKER`], at `/opt/<package>/.roadcase-install`,
//! holding `deb` or `pacman`. The marker belongs to the package, so it comes
//! and goes with it, needs no query of the package database, and works in a
//! prefix of any name. An AppImage never has one beside its executable,
//! since that lives inside the mounted image; a copy started from a build
//! folder has none either.

use crate::manifest::Kind;
use std::path::Path;

/// The marker file's name, at the root of a package's install folder: the
/// parent of the folder that holds the executable, or that folder itself.
pub const INSTALL_MARKER: &str = ".roadcase-install";

/// A system package manager that installed, and updates, the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PackageManager {
    /// apt (dpkg), from a `.deb`.
    Apt,
    /// pacman, from a pacman repository or the AUR.
    Pacman,
}

impl PackageManager {
    /// What the marker file holds for it: `deb` or `pacman`.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Apt => "deb",
            Self::Pacman => "pacman",
        }
    }

    /// The manager whose marker this text is, ignoring surrounding space.
    pub fn from_marker(text: &str) -> Option<Self> {
        match text.lines().next().unwrap_or_default().trim() {
            "deb" => Some(Self::Apt),
            "pacman" => Some(Self::Pacman),
            _ => None,
        }
    }

    /// The kind of artifact that announces its releases.
    pub fn kind(self) -> Kind {
        match self {
            Self::Apt => Kind::Deb,
            Self::Pacman => Kind::Pacman,
        }
    }

    /// The manifest key its releases are announced under, for a platform
    /// such as `linux-x86_64`: `linux-x86_64-deb`, `linux-x86_64-pacman`.
    pub fn target(self, platform: &str) -> String {
        format!("{platform}-{}", self.marker())
    }

    /// The command people know it by: `apt`, `pacman`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Pacman => "pacman",
        }
    }
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The package manager that installed the running application, if one did.
///
/// `None` on systems other than Linux, inside an AppImage (`$APPIMAGE` is
/// set), and when no marker is found beside the executable.
pub fn package_manager() -> Option<PackageManager> {
    if !cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_some() {
        return None;
    }
    // On Linux this is /proc/self/exe: the file, with every link resolved,
    // so /usr/bin/<name> leads to /opt/<package>/bin/<name>.
    let executable = std::env::current_exe().ok()?;
    package_manager_of(&executable)
}

/// The package manager whose marker sits beside `executable`: in its folder,
/// or in that folder's parent.
pub fn package_manager_of(executable: &Path) -> Option<PackageManager> {
    let folder = executable.parent()?;
    std::iter::successors(Some(folder), |folder| folder.parent())
        .take(2)
        .find_map(|folder| {
            let text = std::fs::read_to_string(folder.join(INSTALL_MARKER)).ok()?;
            PackageManager::from_marker(&text)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_names_its_manager() {
        assert_eq!(
            PackageManager::from_marker("deb\n"),
            Some(PackageManager::Apt)
        );
        assert_eq!(
            PackageManager::from_marker(" pacman "),
            Some(PackageManager::Pacman)
        );
        assert_eq!(PackageManager::from_marker("rpm"), None);
        assert_eq!(PackageManager::from_marker(""), None);
        assert_eq!(
            PackageManager::Apt.target("linux-x86_64"),
            "linux-x86_64-deb"
        );
        assert_eq!(PackageManager::Pacman.kind(), Kind::Pacman);
        assert_eq!(PackageManager::Apt.to_string(), "apt");
    }

    #[test]
    fn the_marker_is_found_beside_the_executable_or_one_folder_up() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("opt/example");
        let bin = prefix.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = bin.join("example");
        assert_eq!(package_manager_of(&executable), None);

        std::fs::write(prefix.join(INSTALL_MARKER), "deb\n").unwrap();
        assert_eq!(package_manager_of(&executable), Some(PackageManager::Apt));

        std::fs::write(bin.join(INSTALL_MARKER), "pacman\n").unwrap();
        assert_eq!(
            package_manager_of(&executable),
            Some(PackageManager::Pacman)
        );

        // Two folders up is someone else's.
        let deeper = bin.join("helpers/tool");
        std::fs::create_dir_all(deeper.parent().unwrap()).unwrap();
        std::fs::remove_file(bin.join(INSTALL_MARKER)).unwrap();
        assert_eq!(package_manager_of(&deeper), None);
    }
}
