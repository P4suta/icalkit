// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::PUBLIC_API_TOOLCHAIN;

#[derive(Debug)]
struct PinnedCargo {
    executable: PathBuf,
    bin: PathBuf,
}

impl PinnedCargo {
    fn from_output(output: Output) -> io::Result<Self> {
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "cannot select {PUBLIC_API_TOOLCHAIN} for API generation ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let text = String::from_utf8(output.stdout).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("rustup which emitted non-UTF-8 output: {error}"),
            )
        })?;
        let text = text.trim_end_matches(['\r', '\n']);
        let executable = PathBuf::from(text);
        let name = format!("cargo{}", env::consts::EXE_SUFFIX);
        if !executable.is_absolute()
            || executable.file_name() != Some(OsStr::new(&name))
            || text.contains(['\r', '\n'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "rustup which emitted an invalid Cargo path",
            ));
        }
        let bin = executable
            .parent()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "selected Cargo has no binary directory",
                )
            })?
            .to_owned();
        Ok(Self { executable, bin })
    }

    fn command(
        &self,
        workspace: &Path,
        no_default_features: bool,
        inherited_path: Option<&OsStr>,
    ) -> io::Result<Command> {
        let inherited = inherited_path.into_iter().flat_map(env::split_paths);
        let path = env::join_paths(std::iter::once(self.bin.clone()).chain(inherited))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut command = Command::new(&self.executable);
        command.args(["public-api", "-p", "icalkit", "-sss", "--color", "never"]);
        if no_default_features {
            command.arg("--no-default-features");
        }
        command
            .current_dir(workspace)
            .env("PATH", path)
            .env("CARGO", &self.executable)
            .env("RUSTUP_TOOLCHAIN", PUBLIC_API_TOOLCHAIN)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env(
                "RUSTC",
                self.bin.join(format!("rustc{}", env::consts::EXE_SUFFIX)),
            )
            .env(
                "RUSTDOC",
                self.bin.join(format!("rustdoc{}", env::consts::EXE_SUFFIX)),
            )
            .env("RUSTDOCFLAGS", "-D warnings");
        Ok(command)
    }
}

pub(super) fn command(workspace: &Path, no_default_features: bool) -> io::Result<Command> {
    let output = Command::new("rustup")
        .args(["which", "--toolchain", PUBLIC_API_TOOLCHAIN, "cargo"])
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()?;
    let compiler = PinnedCargo::from_output(output)?;
    let inherited_path = env::var_os("PATH");
    compiler.command(workspace, no_default_features, inherited_path.as_deref())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::PinnedCargo;

    const PROBE_CARGO: &str = "ICALKIT_API_PROBE_CARGO";
    const PROBE_PROFILE: &str = "ICALKIT_API_PROBE_PROFILE";
    const PROBE_TEST: &str = "public_api::tests::api_command_probe";
    const FIXTURE: &str = r#"
use std::io::{self, Write};
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let directory = executable.parent().ok_or("missing executable directory")?;
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--version") => {
            let name = executable.file_stem().ok_or("missing executable name")?.to_string_lossy();
            let version = std::fs::read_to_string(directory.join("version"))?;
            println!("{name} 1.0.0-{version}");
        }
        Some("which") => {
            if std::env::var("RUSTUP_AUTO_INSTALL").as_deref() != Ok("0") {
                eprintln!("fixture: implicit toolchain installation remains enabled");
                std::process::exit(43);
            }
            let selected = std::env::var_os("ICALKIT_API_PROBE_CARGO").unwrap_or_else(|| executable.into_os_string());
            println!("{}\r", std::path::Path::new(&selected).display());
        }
        Some("which-fails") => {
            eprintln!("fixture: dated nightly is unavailable");
            std::process::exit(37);
        }
        _ => {
            if std::env::var("RUSTUP_AUTO_INSTALL").as_deref() != Ok("0") {
                eprintln!("fixture: child toolchain installation remains enabled");
                std::process::exit(44);
            }
            for (name, variable) in [("cargo", "CARGO"), ("rustc", "RUSTC"), ("rustdoc", "RUSTDOC")] {
                let program = if name == "cargo" {
                    name.into()
                } else {
                    std::env::var_os(variable).unwrap_or_else(|| name.into())
                };
                let output = Command::new(program).arg("--version").output()?;
                if !output.status.success() {
                    return Err(format!("{name} failed: {:?}", output).into());
                }
                io::stdout().write_all(&output.stdout)?;
            }
            println!("profile={}", if arguments.iter().any(|arg| arg == "--no-default-features") { "no-default" } else { "default" });
        }
    }
    Ok(())
}
"#;

    struct ToolchainFixture(PathBuf);

    impl ToolchainFixture {
        fn new() -> io::Result<Self> {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "icalkit-api-toolchain-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            let fixture = Self(path);
            let source = fixture.0.join("cargo_fixture.rs");
            fs::write(&source, FIXTURE)?;
            let executable = fixture
                .0
                .join(format!("cargo_fixture{}", std::env::consts::EXE_SUFFIX));
            let output = Command::new("rustc")
                .args(["--edition=2024", "--crate-name", "cargo_fixture"])
                .arg(&source)
                .arg("-o")
                .arg(&executable)
                .output()?;
            assert!(
                output.status.success(),
                "fixture compilation failed: {output:?}"
            );
            for (directory, version) in [
                ("pinned compiler", "nightly"),
                ("foreign compiler", "foreign"),
            ] {
                let bin = fixture.0.join(directory).join("bin");
                fs::create_dir_all(&bin)?;
                fs::write(bin.join("version"), version)?;
                for program in ["cargo", "rustc", "rustdoc", "rustup"] {
                    fs::copy(
                        &executable,
                        bin.join(format!("{program}{}", std::env::consts::EXE_SUFFIX)),
                    )?;
                }
            }
            Ok(fixture)
        }

        fn program(&self, directory: &str, program: &str) -> PathBuf {
            self.0
                .join(directory)
                .join("bin")
                .join(format!("{program}{}", std::env::consts::EXE_SUFFIX))
        }

        fn which(&self) -> io::Result<Output> {
            Command::new(self.program("pinned compiler", "cargo"))
                .arg("which")
                .env("RUSTUP_AUTO_INSTALL", "0")
                .output()
        }
    }

    impl Drop for ToolchainFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove owned toolchain fixture");
        }
    }

    #[test]
    fn child_tool_detection_uses_the_selected_compiler_with_a_foreign_environment() -> io::Result<()>
    {
        let fixture = ToolchainFixture::new()?;
        let inherited = std::env::var_os("PATH");
        let paths = std::iter::once(fixture.0.join("foreign compiler").join("bin")).chain(
            inherited
                .as_deref()
                .into_iter()
                .flat_map(std::env::split_paths),
        );
        let search = std::env::join_paths(paths).map_err(io::Error::other)?;
        for profile in ["default", "no-default"] {
            let output = Command::new(std::env::current_exe()?)
                .args(["--exact", PROBE_TEST, "--nocapture"])
                .env(PROBE_CARGO, fixture.program("pinned compiler", "cargo"))
                .env(PROBE_PROFILE, profile)
                .env("PATH", &search)
                .env("CARGO", fixture.program("foreign compiler", "cargo"))
                .env("RUSTC", fixture.program("foreign compiler", "rustc"))
                .env("RUSTDOC", fixture.program("foreign compiler", "rustdoc"))
                .env("RUSTUP_TOOLCHAIN", "unrelated-toolchain")
                .env("RUSTUP_AUTO_INSTALL", "1")
                .output()?;
            assert!(
                output.status.success(),
                "selected compiler lost to inherited environment: {output:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn api_command_probe() -> io::Result<()> {
        if std::env::var_os(PROBE_CARGO).is_none() {
            return Ok(());
        }
        let profile = std::env::var(PROBE_PROFILE).map_err(io::Error::other)?;
        let output = super::command(Path::new("."), profile == "no-default")?.output()?;
        assert!(
            output.status.success(),
            "selected compiler command failed: {output:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!(
                "cargo 1.0.0-nightly\nrustc 1.0.0-nightly\nrustdoc 1.0.0-nightly\nprofile={profile}\n"
            )
        );
        Ok(())
    }

    #[test]
    fn unavailable_compilers_preserve_the_selection_failure() -> io::Result<()> {
        let fixture = ToolchainFixture::new()?;
        let output = Command::new(fixture.program("pinned compiler", "cargo"))
            .arg("which-fails")
            .output()?;
        let error = PinnedCargo::from_output(output).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(super::super::PUBLIC_API_TOOLCHAIN)
        );
        assert!(error.to_string().contains("37"));
        assert!(error.to_string().contains("dated nightly is unavailable"));
        Ok(())
    }

    #[test]
    fn malformed_compiler_paths_fail_before_launch() -> io::Result<()> {
        let fixture = ToolchainFixture::new()?;
        let valid = fixture.which()?;
        for path in [
            b"".as_slice(),
            b"relative/cargo\n",
            b"\xff\n",
            b"/cargo\n/other\n",
        ] {
            let mut output = valid.clone();
            output.stdout = path.to_vec();
            assert_eq!(
                PinnedCargo::from_output(output).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        let compiler = PinnedCargo::from_output(valid)?;
        assert!(
            compiler
                .command(Path::new("."), false, Some(OsStr::new("")))
                .is_ok()
        );
        Ok(())
    }
}
