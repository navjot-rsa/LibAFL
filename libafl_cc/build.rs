#[cfg(target_vendor = "apple")]
use glob::glob;
#[cfg(target_vendor = "apple")]
use std::path::PathBuf;
use std::{env, fs::File, io::Write, path::Path, process::Command, str};
#[cfg(not(target_vendor = "apple"))]
use which::which;

/// The max version of `LLVM` we're looking for
#[cfg(not(target_vendor = "apple"))]
const LLVM_VERSION_MAX: u32 = 33;

/// The min version of `LLVM` we're looking for
#[cfg(not(target_vendor = "apple"))]
const LLVM_VERSION_MIN: u32 = 6;

/// Get the extension for a shared object
fn dll_extension<'a>() -> &'a str {
    match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "windwos" => "dll",
        "macos" | "ios" => "dylib",
        _ => "so",
    }
}

/// Github Actions for `MacOS` seems to have troubles finding `llvm-config`.
/// Hence, we go look for it ourselves.
#[cfg(target_vendor = "apple")]
fn find_llvm_config_brew() -> Result<PathBuf, String> {
    match Command::new("brew").arg("--cellar").output() {
        Ok(output) => {
            let brew_cellar_location = str::from_utf8(&output.stdout).unwrap_or_default().trim();
            if brew_cellar_location.is_empty() {
                return Err("Empty return from brew --cellar".to_string());
            }
            let cellar_glob = format!("{}/llvm/*/bin/llvm-config", brew_cellar_location);
            let glob_results = glob(&cellar_glob).unwrap_or_else(|err| {
                panic!("Could not read glob path {} ({})", &cellar_glob, err);
            });
            match glob_results.last() {
                Some(path) => Ok(path.unwrap()),
                None => Err(format!(
                    "No llvm-config found in brew cellar with pattern {}",
                    cellar_glob
                )),
            }
        }
        Err(err) => Err(format!("Could not execute brew --cellar: {:?}", err)),
    }
}

fn find_llvm_config() -> String {
    env::var("LLVM_CONFIG").unwrap_or_else(|_| {
        // for Ghithub Actions, we check if we find llvm-config in brew.
        #[cfg(target_vendor = "apple")]
        match find_llvm_config_brew() {
            Ok(llvm_dir) => llvm_dir.to_str().unwrap().to_string(),
            Err(err) => {
                println!("cargo:warning={}", err);
                // falling back to system llvm-config
                "llvm-config".to_string()
            }
        }
        #[cfg(not(target_vendor = "apple"))]
        for version in (LLVM_VERSION_MIN..=LLVM_VERSION_MAX).rev() {
            let llvm_config_name = format!("llvm-config-{}", version);
            if which(&llvm_config_name).is_ok() {
                return llvm_config_name;
            }
        }
        #[cfg(not(target_vendor = "apple"))]
        "llvm-config".to_string()
    })
}

#[allow(clippy::too_many_lines)]
fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);
    let src_dir = Path::new("src");

    println!("cargo:rerun-if-env-changed=LLVM_CONFIG");
    println!("cargo:rerun-if-env-changed=LIBAFL_EDGES_MAP_SIZE");
    println!("cargo:rerun-if-env-changed=LIBAFL_ACCOUNTING_MAP_SIZE");

    let mut custom_flags = vec![];

    let dest_path = Path::new(&out_dir).join("clang_constants.rs");
    let mut clang_constants_file = File::create(&dest_path).expect("Could not create file");

    let edges_map_size: usize = option_env!("LIBAFL_EDGES_MAP_SIZE")
        .map_or(Ok(65536), str::parse)
        .expect("Could not parse LIBAFL_EDGES_MAP_SIZE");
    custom_flags.push(format!("-DLIBAFL_EDGES_MAP_SIZE={}", edges_map_size));

    let acc_map_size: usize = option_env!("LIBAFL_ACCOUNTING_MAP_SIZE")
        .map_or(Ok(65536), str::parse)
        .expect("Could not parse LIBAFL_ACCOUNTING_MAP_SIZE");
    custom_flags.push(format!("-DLIBAFL_ACCOUNTING_MAP_SIZE={}", acc_map_size));

    let llvm_config = find_llvm_config();

    if let Ok(output) = Command::new(&llvm_config).args(&["--bindir"]).output() {
        let llvm_bindir = Path::new(
            str::from_utf8(&output.stdout)
                .expect("Invalid llvm-config output")
                .trim(),
        );
        write!(
            clang_constants_file,
            "// These constants are autogenerated by build.rs

            /// The path to the `clang` executable
            pub const CLANG_PATH: &str = {:?};
            /// The path to the `clang++` executable
            pub const CLANGXX_PATH: &str = {:?};
            
            /// The size of the edges map
            pub const EDGES_MAP_SIZE: usize = {};

            /// The size of the accounting maps
            pub const ACCOUNTING_MAP_SIZE: usize = {};
            ",
            llvm_bindir.join("clang"),
            llvm_bindir.join("clang++"),
            edges_map_size,
            acc_map_size
        )
        .expect("Could not write file");

        let output = Command::new(&llvm_config)
            .args(&["--cxxflags"])
            .output()
            .expect("Failed to execute llvm-config");
        let cxxflags = str::from_utf8(&output.stdout).expect("Invalid llvm-config output");

        let mut cmd = Command::new(&llvm_config);

        #[cfg(target_vendor = "apple")]
        {
            cmd.args(&["--libs"]);
        }

        let output = cmd
            .args(&["--ldflags"])
            .output()
            .expect("Failed to execute llvm-config");
        let ldflags = str::from_utf8(&output.stdout).expect("Invalid llvm-config output");

        let cxxflags: Vec<&str> = cxxflags.trim().split_whitespace().collect();
        let mut ldflags: Vec<&str> = ldflags.trim().split_whitespace().collect();

        if env::var("CARGO_CFG_TARGET_VENDOR").unwrap().as_str() == "apple" {
            // Needed on macos.
            // Explanation at https://github.com/banach-space/llvm-tutor/blob/787b09ed31ff7f0e7bdd42ae20547d27e2991512/lib/CMakeLists.txt#L59
            ldflags.push("-undefined");
            ldflags.push("dynamic_lookup");
        };

        println!("cargo:rerun-if-changed=src/common-llvm.h");
        println!("cargo:rerun-if-changed=src/cmplog-routines-pass.cc");
        println!("cargo:rerun-if-changed=src/afl-coverage-pass.cc");
        println!("cargo:rerun-if-changed=src/autotokens-pass.cc");
        println!("cargo:rerun-if-changed=src/coverage-accounting-pass.cc");

        assert!(Command::new(llvm_bindir.join("clang++"))
            .args(&cxxflags)
            .args(&custom_flags)
            .arg(src_dir.join("cmplog-routines-pass.cc"))
            .args(&ldflags)
            .args(&["-fPIC", "-shared", "-o"])
            .arg(out_dir.join(format!("cmplog-routines-pass.{}", dll_extension())))
            .status()
            .expect("Failed to compile cmplog-routines-pass.cc")
            .success());

        assert!(Command::new(llvm_bindir.join("clang++"))
            .args(&cxxflags)
            .args(&custom_flags)
            .arg(src_dir.join("afl-coverage-pass.cc"))
            .args(&ldflags)
            .args(&["-fPIC", "-shared", "-o"])
            .arg(out_dir.join(format!("afl-coverage-pass.{}", dll_extension())))
            .status()
            .expect("Failed to compile afl-coverage-pass.cc")
            .success());

        assert!(Command::new(llvm_bindir.join("clang++"))
            .args(&cxxflags)
            .args(&custom_flags)
            .arg(src_dir.join("autotokens-pass.cc"))
            .args(&ldflags)
            .args(&["-fPIC", "-shared", "-o"])
            .arg(out_dir.join(format!("autotokens-pass.{}", dll_extension())))
            .status()
            .expect("Failed to compile autotokens-pass.cc")
            .success());

        assert!(Command::new(llvm_bindir.join("clang++"))
            .args(&cxxflags)
            .args(&custom_flags)
            .arg(src_dir.join("coverage-accounting-pass.cc"))
            .args(&ldflags)
            .args(&["-fPIC", "-shared", "-o"])
            .arg(out_dir.join(format!("coverage-accounting-pass.{}", dll_extension())))
            .status()
            .expect("Failed to compile coverage-accounting-pass.cc")
            .success());
    } else {
        write!(
            clang_constants_file,
            "// These constants are autogenerated by build.rs

/// The path to the `clang` executable
pub const CLANG_PATH: &str = \"clang\";
/// The path to the `clang++` executable
pub const CLANGXX_PATH: &str = \"clang++\";
    "
        )
        .expect("Could not write file");

        println!(
            "cargo:warning=Failed to locate the LLVM path using {}, we will not build LLVM passes
            (if you need them, set point the LLVM_CONFIG env to a recent llvm-config, or make sure {} is available)",
            llvm_config, llvm_config
        );
    }

    cc::Build::new()
        .file(src_dir.join("no-link-rt.c"))
        .compile("no-link-rt");

    println!("cargo:rerun-if-changed=build.rs");
}
