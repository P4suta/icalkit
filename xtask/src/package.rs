// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::process::Command;

use super::candidate::ScratchDirectory;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) fn check(workspace: &Path, arguments: &[String]) -> io::Result<Vec<String>> {
    let version = std::env::var_os("ICALKIT_RELEASE_VERSION");
    let archive = std::env::var_os("ICALKIT_RELEASE_ARCHIVE");
    check_with_release(
        workspace,
        arguments,
        version
            .as_deref()
            .filter(|version| !version.is_empty())
            .map(std::ffi::OsStr::to_string_lossy)
            .as_deref(),
        archive
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(Path::new),
    )
}

fn check_with_release(
    workspace: &Path,
    arguments: &[String],
    version: Option<&str>,
    archive: Option<&Path>,
) -> io::Result<Vec<String>> {
    let allow_dirty = match arguments {
        [] => false,
        [flag] if flag == "--allow-dirty" => true,
        _ => return Ok(vec!["package-check accepts only --allow-dirty".to_owned()]),
    };
    if let Some(expected) = version {
        check_release_version(VERSION, expected)?;
        if allow_dirty {
            return Ok(vec![
                "release qualification requires a clean checkout".to_owned(),
            ]);
        }
    }
    if let Some(path) = archive {
        if version.is_none() {
            return Ok(vec![
                "archive retention requires a release version".to_owned(),
            ]);
        }
        if !path.is_absolute() {
            return Ok(vec![
                "archive retention requires an absolute path".to_owned(),
            ]);
        }
    }
    let scratch = ScratchDirectory::create(&std::env::temp_dir(), "package")?;
    let mut package = cargo(workspace, scratch.path());
    package.args(["package", "--locked", "-p", "icalkit"]);
    if allow_dirty {
        package.arg("--allow-dirty");
    }
    run(&mut package, "package the crate")?;

    let extracted = scratch
        .path()
        .join("package")
        .join(format!("icalkit-{VERSION}"));
    verify_contents(workspace, &extracted)?;
    run(
        cargo(&extracted, scratch.path()).args(["test", "--locked", "--all-features", "--quiet"]),
        "test the extracted crate",
    )?;
    run(
        cargo(&extracted, scratch.path()).args([
            "run",
            "--locked",
            "--no-default-features",
            "--example",
            "golden_path",
        ]),
        "run the extracted example",
    )?;
    if let Some(destination) = archive {
        retain_archive(
            &extracted.with_file_name(format!("icalkit-{VERSION}.crate")),
            destination,
        )?;
    }
    Ok(Vec::new())
}

fn retain_archive(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source = File::open(source)?;
    let mut archive = File::create_new(destination)?;
    io::copy(&mut source, &mut archive)?;
    Ok(())
}

fn check_release_version(actual: &str, expected: &str) -> io::Result<()> {
    if actual != "0.0.0" && actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "release version {expected:?} must match a publishable icalkit version, found {actual}"
        )))
    }
}

fn cargo(directory: &Path, target: &Path) -> Command {
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(directory)
        .env("CARGO_TARGET_DIR", target);
    command
}

fn run(command: &mut Command, operation: &str) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("could not {operation}: {status}")))
    }
}

fn verify_contents(workspace: &Path, extracted: &Path) -> io::Result<()> {
    for name in ["LICENSE-APACHE", "LICENSE-MIT", "README.md"] {
        check_copy(&workspace.join(name), &extracted.join(name))?;
    }
    let mirrors = workspace.join("crates/icalkit/tests/fixtures/conformance");
    let originals = workspace.join("crates/icalkit-conformance/tests/fixtures");
    let packaged = extracted.join("tests/fixtures/conformance");
    for directory in ["itip", "break_itip_methods", "break_itip_sequence"] {
        for entry in fs::read_dir(mirrors.join(directory))? {
            let relative = Path::new(directory).join(entry?.file_name());
            check_copy(&originals.join(&relative), &packaged.join(relative))?;
        }
    }
    Ok(())
}

fn check_copy(original: &Path, copy: &Path) -> io::Result<()> {
    let bytes = fs::read(copy)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", copy.display())))?;
    if fs::read(original)? == bytes {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} differs from {}",
            copy.display(),
            original.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::check_release_version;

    #[test]
    fn archive_retention_requires_release_qualification() {
        use std::path::Path;

        assert_eq!(
            super::check_with_release(Path::new("unused"), &[], None, Some(Path::new("crate")))
                .unwrap(),
            ["archive retention requires a release version"]
        );
        assert_eq!(
            super::check_with_release(
                Path::new("unused"),
                &[],
                Some(super::VERSION),
                Some(Path::new("crate")),
            )
            .unwrap(),
            ["archive retention requires an absolute path"]
        );
    }

    #[test]
    fn retaining_an_archive_copies_its_bytes_and_preserves_an_existing_file() {
        let directory =
            super::ScratchDirectory::create(&std::env::temp_dir(), "retain-archive").unwrap();
        let source = directory.path().join("source.crate");
        let destination = directory.path().join("qualified.crate");
        std::fs::write(&source, b"verified archive").unwrap();
        super::retain_archive(&source, &destination).unwrap();
        std::fs::write(&source, b"different archive").unwrap();
        let error = super::retain_archive(&source, &destination).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(destination).unwrap(), b"verified archive");
    }

    #[test]
    fn release_version_must_match_and_exclude_the_unreleased_version() {
        assert!(check_release_version("0.2.3", "0.2.3").is_ok());
        assert!(check_release_version("0.2.3", "0.2.4").is_err());
        assert!(check_release_version("0.2.3", "").is_err());
        assert!(check_release_version("0.0.0", "0.0.0").is_err());
    }
}
