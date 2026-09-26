use std::{env, fs, path::PathBuf};

const BINARIES: [&str; 4] = ["sshd", "sshd-auth", "sshd-session", "ssh-keygen"];

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must provide target arch");
    let expected_machine = match target_arch.as_str() {
        "x86_64" => 62,
        "aarch64" => 183,
        other => panic!("ssh_precreate_hook does not support target architecture {other}"),
    };
    let assets = PathBuf::from("assets");

    for name in BINARIES {
        let path = assets.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "missing bundled {name} at {}: {error}; run scripts/build-ssh-precreate-hook.sh",
                path.display()
            )
        });
        validate_elf(name, &bytes, expected_machine, &target_arch);
    }
}

fn validate_elf(name: &str, bytes: &[u8], expected_machine: u16, target_arch: &str) {
    assert!(
        bytes.len() >= 20 && &bytes[..4] == b"\x7fELF",
        "bundled {name} is not an ELF executable"
    );
    assert_eq!(bytes[4], 2, "bundled {name} is not a 64-bit ELF executable");
    assert_eq!(
        bytes[5], 1,
        "bundled {name} is not a little-endian ELF executable"
    );
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    assert_eq!(
        machine, expected_machine,
        "bundled {name} does not match Cargo target architecture {target_arch}; run scripts/build-ssh-precreate-hook.sh on a native {target_arch} builder"
    );
}
