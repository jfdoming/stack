#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const INSTALLER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh");

struct InstallerFixture {
    root: TempDir,
    work: PathBuf,
    home: PathBuf,
    fake_bin: PathBuf,
    cargo_log: PathBuf,
}

impl InstallerFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let work = root.path().join("work");
        let home = root.path().join("home");
        let fake_bin = root.path().join("fake-bin");
        let cargo_log = root.path().join("cargo.log");
        fs::create_dir_all(&work).expect("create work dir");
        fs::create_dir_all(&home).expect("create home dir");
        fs::create_dir_all(&fake_bin).expect("create fake bin dir");

        let fake_cargo = fake_bin.join("cargo");
        fs::write(
            &fake_cargo,
            "#!/bin/sh\n\
             set -eu\n\
             printf '%s\\n' \"$*\" > \"$FAKE_CARGO_LOG\"\n\
             mkdir -p target/release\n\
             printf '#!/bin/sh\\nexit 0\\n' > target/release/stack\n\
             chmod 755 target/release/stack\n",
        )
        .expect("write fake cargo");
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755))
            .expect("chmod fake cargo");

        Self {
            root,
            work,
            home,
            fake_bin,
            cargo_log,
        }
    }

    fn command(&self, prefix: &Path, shell: &str) -> Command {
        let mut command = Command::new("bash");
        command
            .arg(INSTALLER)
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("SHELL", shell)
            .env("STACK_INSTALL_PREFIX", prefix)
            .env("FAKE_CARGO_LOG", &self.cargo_log)
            .env("PATH", format!("{}:/usr/bin:/bin", self.fake_bin.display()));
        command
    }
}

fn run(command: &mut Command) -> Output {
    command.output().expect("run installer")
}

#[test]
fn installer_rejects_unknown_arguments_before_building() {
    for args in [vec!["--bogus"], vec!["--write-shell-config", "unexpected"]] {
        let fixture = InstallerFixture::new();
        let prefix = fixture.root.path().join("prefix");
        let output = run(fixture.command(&prefix, "/bin/bash").args(&args));

        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unknown argument"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !fixture.cargo_log.exists(),
            "invalid arguments must fail before cargo runs"
        );
        assert!(!prefix.exists(), "invalid arguments must not install files");
    }
}

#[test]
fn installer_uses_locked_build_and_writes_safe_bash_and_zsh_config() {
    for (shell, rc_name) in [("/bin/bash", ".bashrc"), ("/bin/zsh", ".zshrc")] {
        let fixture = InstallerFixture::new();
        let prefix = fixture
            .root
            .path()
            .join(r#"install $(touch PWNED) 'quote' \slash $dollar"#);
        let output = run(fixture.command(&prefix, shell).arg("--write-shell-config"));
        assert!(
            output.status.success(),
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&fixture.cargo_log).expect("read cargo log"),
            "build --release --locked\n"
        );

        let rc_file = fixture.home.join(rc_name);
        let source_output = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                "source \"$RC_FILE\"; printf '%s' \"${PATH%%:*}\"",
            ])
            .current_dir(fixture.root.path())
            .env("RC_FILE", &rc_file)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("source generated shell config");
        assert!(
            source_output.status.success(),
            "generated config failed: {}",
            String::from_utf8_lossy(&source_output.stderr)
        );
        assert_eq!(
            String::from_utf8(source_output.stdout).expect("UTF-8 PATH"),
            prefix.join("bin").to_string_lossy()
        );
        assert!(
            !fixture.root.path().join("PWNED").exists(),
            "generated config executed prefix contents"
        );
        assert!(prefix.join("bin/stack").is_file());
    }
}

#[test]
fn installer_creates_and_idempotently_quotes_fish_config() {
    let fixture = InstallerFixture::new();
    let prefix = fixture
        .root
        .path()
        .join(r#"fish prefix 'quote' \slash $dollar (touch PWNED)"#);

    for _ in 0..2 {
        let output = run(fixture
            .command(&prefix, "/usr/bin/fish")
            .arg("--write-shell-config"));
        assert!(
            output.status.success(),
            "installer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let config = fixture.home.join(".config/fish/config.fish");
    let bin_dir = prefix.join("bin").to_string_lossy().into_owned();
    let escaped = bin_dir.replace('\\', "\\\\").replace('\'', "\\'");
    assert_eq!(
        fs::read_to_string(config).expect("read fish config"),
        format!("fish_add_path -- '{escaped}'\n")
    );
}

#[test]
fn installer_rejects_unrepresentable_path_components_before_building() {
    for suffix in ["bad:prefix", "bad\nprefix", "bad\rprefix"] {
        let fixture = InstallerFixture::new();
        let prefix = fixture.root.path().join(suffix);
        let output = run(&mut fixture.command(&prefix, "/bin/bash"));

        assert!(
            !output.status.success(),
            "prefix should be rejected: {suffix:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot contain"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !fixture.cargo_log.exists(),
            "invalid prefix must fail before cargo runs"
        );
    }
}
