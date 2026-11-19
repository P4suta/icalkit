// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::PUBLIC_API_TOOLCHAIN;

#[derive(Debug)]
struct PinnedCargo {
    executable: PathBuf,
    bin: PathBuf,
}

pub(super) struct ApiGeneration {
    rustdoc: Command,
    formatter: Command,
    json: PathBuf,
    _directory: ApiTargetDirectory,
}

struct ApiTargetDirectory(PathBuf);

impl ApiTargetDirectory {
    fn new(workspace: &Path, no_default_features: bool) -> io::Result<Self> {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let parent = std::path::absolute(workspace)?
            .join("target/public-api")
            .join(if no_default_features {
                "no-default"
            } else {
                "default"
            });
        fs::create_dir_all(&parent)?;
        for _ in 0..8 {
            let directory = parent.join(format!(
                "run-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&directory) {
                Ok(()) => return Ok(Self(directory)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("no unused API generation directory"))
    }
}

impl Drop for ApiTargetDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!(
                "could not remove API generation directory {}: {error}",
                self.0.display()
            );
        }
    }
}

impl ApiGeneration {
    pub(super) fn output(&mut self) -> io::Result<Output> {
        match fs::remove_file(&self.json) {
            Ok(()) => {},
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => return Err(error),
        }
        let generated = self.rustdoc.output()?;
        if !generated.status.success() {
            return Ok(generated);
        }
        self.formatter.output()
    }
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
    ) -> io::Result<ApiGeneration> {
        let host = self.host()?;
        let directory = ApiTargetDirectory::new(workspace, no_default_features)?;
        let json = directory.0.join(&host).join("doc/icalkit.json");
        let mut rustdoc = self.base_command(workspace, inherited_path)?;
        rustdoc
            .args(["rustdoc", "--locked", "--lib", "-p", "icalkit", "--target"])
            .arg(&host)
            .arg("--target-dir")
            .arg(&directory.0);
        if no_default_features {
            rustdoc.arg("--no-default-features");
        }
        rustdoc.args([
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
            "--cap-lints",
            "deny",
        ]);
        let mut formatter = self.base_command(workspace, inherited_path)?;
        formatter
            .args(["public-api", "-sss", "--color", "never", "--rustdoc-json"])
            .arg(&json);
        Ok(ApiGeneration {
            rustdoc,
            formatter,
            json,
            _directory: directory,
        })
    }

    fn host(&self) -> io::Result<String> {
        let output = Command::new(self.bin.join(format!("rustc{}", env::consts::EXE_SUFFIX)))
            .arg("-vV")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "cannot identify the pinned compiler host: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let text = String::from_utf8(output.stdout).map_err(io::Error::other)?;
        let mut hosts = text.lines().filter_map(|line| line.strip_prefix("host: "));
        let host = hosts.next().filter(|host| {
            !host.is_empty()
                && host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        });
        match (host, hosts.next()) {
            (Some(host), None) => Ok(host.to_owned()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pinned Rustc reported an invalid host",
            )),
        }
    }

    fn base_command(
        &self,
        workspace: &Path,
        inherited_path: Option<&OsStr>,
    ) -> io::Result<Command> {
        let inherited = inherited_path.into_iter().flat_map(env::split_paths);
        let path = env::join_paths(std::iter::once(self.bin.clone()).chain(inherited))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut command = Command::new(&self.executable);
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
            .env("RUSTDOCFLAGS", "-D warnings")
            .env_remove("CARGO_BUILD_TARGET")
            .env_remove("CARGO_ENCODED_RUSTDOCFLAGS");
        Ok(command)
    }
}

pub(super) fn command(workspace: &Path, no_default_features: bool) -> io::Result<ApiGeneration> {
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
    const PROBE_SOURCE: &str = "ICALKIT_API_PROBE_SOURCE";
    const PROBE_RUSTDOC: &str = "ICALKIT_API_PROBE_RUSTDOC";
    const PROBE_WORKSPACE: &str = "ICALKIT_API_PROBE_WORKSPACE";
    const PROBE_TEST: &str = "public_api::tests::api_command_probe";
    const FIXTURE: &str = r#"
use std::io::{self, Write};
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let directory = executable.parent().ok_or("missing executable directory")?;
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("-vV") => println!("rustc 1.0.0-nightly\nhost: fixture-host"),
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
            let argument = |name: &str| arguments.windows(2).find_map(|pair| {
                (pair[0] == name).then_some(pair[1].as_str())
            });
            match std::env::var("ICALKIT_API_PROBE_BOUNDARY").as_deref() {
                Ok("locked") if arguments[0] == "public-api" && argument("--rustdoc-json").is_none() => {
                    eprintln!("fixture: API generation delegated to unlocked Cargo");
                    std::process::exit(45);
                }
                Ok("locked") if arguments[0] == "rustdoc" && !arguments.iter().take_while(|arg| *arg != "--").any(|arg| arg == "--locked") => {
                    eprintln!("fixture: Rustdoc did not enforce the lockfile");
                    std::process::exit(46);
                }
                Ok("target") if std::env::var_os("CARGO_BUILD_TARGET").is_some() => {
                    eprintln!("fixture: inherited build target controls API output");
                    std::process::exit(47);
                }
                Ok("target") if arguments[0] == "rustdoc" && argument("--target") != Some("fixture-host") => {
                    eprintln!("fixture: configured build target was not overridden");
                    std::process::exit(48);
                }
                _ => {}
            }
            if arguments[0] == "public-api" {
                if let Some(json) = argument("--rustdoc-json") {
                    io::stdout().write_all(&std::fs::read(json)?)?;
                    return Ok(());
                }
            }
            if let Some(source) = std::env::var_os("ICALKIT_API_PROBE_SOURCE") {
                let flags = match std::env::var("CARGO_ENCODED_RUSTDOCFLAGS") {
                    Ok(encoded) => encoded.split('\u{1f}').map(String::from).collect::<Vec<_>>(),
                    Err(_) => std::env::var("RUSTDOCFLAGS")?.split_whitespace().map(String::from).collect(),
                };
                let cap = arguments.windows(2).find_map(|pair| {
                    (pair[0] == "--cap-lints").then_some(pair[1].as_str())
                }).unwrap_or("warn");
                let status = Command::new(std::env::var_os("ICALKIT_API_PROBE_RUSTDOC").ok_or("missing Rustdoc")?)
                    .args(["--crate-name", "warning_probe", "--crate-type", "lib", "--edition=2024"])
                    .args(flags)
                    .args(["--cap-lints", cap])
                    .arg("--out-dir").arg(directory.join("docs"))
                    .arg(source).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
            let mut generated = Vec::new();
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
                generated.extend_from_slice(&output.stdout);
            }
            writeln!(generated, "profile={}", if arguments.iter().any(|arg| arg == "--no-default-features") { "no-default" } else { "default" })?;
            if arguments[0] == "rustdoc" {
                let output = std::path::Path::new(argument("--target-dir").ok_or("missing target directory")?)
                    .join(argument("--target").ok_or("missing target")?).join("doc");
                std::fs::create_dir_all(&output)?;
                std::fs::write(output.join("icalkit.json"), generated)?;
            } else {
                io::stdout().write_all(&generated)?;
            }
        }
    }
    Ok(())
}
"#;

    struct ToolchainFixture(PathBuf);

    impl ToolchainFixture {
        fn new() -> io::Result<Self> {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let path = std::path::absolute(std::env::temp_dir())?.join(format!(
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
                .env(PROBE_WORKSPACE, &fixture.0)
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
        let workspace =
            std::env::var_os(PROBE_WORKSPACE).map_or_else(|| PathBuf::from("."), PathBuf::from);
        let output = super::command(&workspace, profile == "no-default")?.output()?;
        if std::env::var_os(PROBE_SOURCE).is_some() {
            assert!(
                !output.status.success(),
                "API generation accepted a documentation warning: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unresolved link to `Missing`"),
                "API generation lost the Rustdoc diagnostic: {output:?}"
            );
            return Ok(());
        }
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

    fn check_documentation_warning(encoded_flags: Option<&str>) -> io::Result<()> {
        let fixture = ToolchainFixture::new()?;
        let source = fixture.0.join("warning.rs");
        fs::write(
            &source,
            "//! A link to [Missing] must fail documentation verification.\n\npub struct Present;\n",
        )?;
        let output = Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()?;
        assert!(output.status.success(), "cannot locate Rustdoc: {output:?}");
        let sysroot = String::from_utf8(output.stdout).map_err(io::Error::other)?;
        let rustdoc = Path::new(sysroot.trim_end_matches(['\r', '\n']))
            .join("bin")
            .join(format!("rustdoc{}", std::env::consts::EXE_SUFFIX));
        let inherited = std::env::var_os("PATH");
        let paths = std::iter::once(fixture.0.join("pinned compiler").join("bin")).chain(
            inherited
                .as_deref()
                .into_iter()
                .flat_map(std::env::split_paths),
        );
        let search = std::env::join_paths(paths).map_err(io::Error::other)?;
        for profile in ["default", "no-default"] {
            let mut probe = Command::new(std::env::current_exe()?);
            probe
                .args(["--exact", PROBE_TEST, "--nocapture"])
                .env(PROBE_CARGO, fixture.program("pinned compiler", "cargo"))
                .env(PROBE_PROFILE, profile)
                .env(PROBE_SOURCE, &source)
                .env(PROBE_RUSTDOC, &rustdoc)
                .env("PATH", &search)
                .env(PROBE_WORKSPACE, &fixture.0)
                .env_remove("CARGO_ENCODED_RUSTDOCFLAGS");
            if let Some(flags) = encoded_flags {
                probe.env("CARGO_ENCODED_RUSTDOCFLAGS", flags);
            }
            let output = probe.output()?;
            assert!(
                output.status.success(),
                "warning policy failed for {profile}: {output:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn documentation_warnings_refuse_api_generation() -> io::Result<()> {
        check_documentation_warning(None)
    }

    #[test]
    fn encoded_flags_do_not_replace_api_warning_policy() -> io::Result<()> {
        check_documentation_warning(Some("-A\u{1f}warnings"))
    }

    fn check_generation_boundary(boundary: &str) -> io::Result<()> {
        let fixture = ToolchainFixture::new()?;
        let search = std::env::join_paths(
            std::iter::once(fixture.0.join("pinned compiler").join("bin")).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ),
        )
        .map_err(io::Error::other)?;
        fs::create_dir(fixture.0.join(".cargo"))?;
        fs::write(
            fixture.0.join(".cargo/config.toml"),
            "[build]\ntarget = \"unavailable-configured-target\"\n",
        )?;
        for profile in ["default", "no-default"] {
            let output = Command::new(std::env::current_exe()?)
                .args(["--exact", PROBE_TEST, "--nocapture"])
                .env(PROBE_CARGO, fixture.program("pinned compiler", "cargo"))
                .env(PROBE_WORKSPACE, &fixture.0)
                .env(PROBE_PROFILE, profile)
                .env("PATH", &search)
                .env("ICALKIT_API_PROBE_BOUNDARY", boundary)
                .env("CARGO_BUILD_TARGET", "unavailable-environment-target")
                .output()?;
            assert!(
                output.status.success(),
                "{boundary} boundary failed for {profile}: {output:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn api_generation_enforces_the_inner_cargo_lockfile() -> io::Result<()> {
        check_generation_boundary("locked")
    }

    #[test]
    fn a_failed_overlapping_generation_keeps_the_other_output() -> io::Result<()> {
        let fixture = ToolchainFixture::new()?;
        let compiler = PinnedCargo::from_output(fixture.which()?)?;
        for no_default_features in [false, true] {
            let mut first = compiler.command(&fixture.0, no_default_features, None)?;
            let mut second = compiler.command(&fixture.0, no_default_features, None)?;
            let generated = first.rustdoc.output()?;
            assert!(generated.status.success(), "{generated:?}");
            second.rustdoc.env("RUSTUP_AUTO_INSTALL", "1");
            let refused = second.output()?;
            assert_eq!(refused.status.code(), Some(44));
            let formatted = first.formatter.output()?;
            assert!(formatted.status.success(), "{formatted:?}");
            assert!(
                String::from_utf8_lossy(&formatted.stdout).contains(if no_default_features {
                    "profile=no-default"
                } else {
                    "profile=default"
                })
            );
        }
        Ok(())
    }

    #[test]
    fn api_generation_uses_the_native_target_json_despite_external_target_settings()
    -> io::Result<()> {
        check_generation_boundary("target")
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
