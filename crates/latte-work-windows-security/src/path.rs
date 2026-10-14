//! Validate the original spelling before Win32 can normalize aliases. Never canonicalize links.
use std::{
    io,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, PathBuf, Prefix},
};

pub(crate) fn control_path(path: &Path) -> io::Result<PathBuf> {
    let units: Vec<_> = path.as_os_str().encode_wide().collect();
    if !path.is_absolute() || units.len() > 32_000 || units.contains(&0) {
        return Err(invalid());
    }
    // Path::components normalizes '.', so inspect the original spelling first.
    if units
        .split(|u| matches!(*u, 47 | 92))
        .any(|part| part == [46] || part == [46, 46])
    {
        return Err(invalid());
    }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(invalid());
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) if drive.is_ascii_alphabetic() => drive,
        _ => return Err(invalid()),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(invalid());
    }
    let mut output = PathBuf::from(format!(r"\\?\{}:\", char::from(drive)));
    let mut count = 0;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid());
        };
        let units: Vec<_> = name.encode_wide().collect();
        if units
            .iter()
            .any(|u| *u < 32 || matches!(*u, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
            || units.last().is_some_and(|u| matches!(*u, 32 | 46))
        {
            return Err(invalid());
        }
        // Lossy text is used only for ASCII device-name rejection. The returned
        // path is assembled from the original OsStr, preserving native UTF-16.
        let text = name.to_string_lossy();
        let stem = text
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ')
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        }) {
            return Err(invalid());
        }
        output.push(name);
        count += 1;
    }
    if count == 0 {
        return Err(invalid());
    }
    Ok(output)
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "control directory requires an unambiguous absolute local disk path",
    )
}
