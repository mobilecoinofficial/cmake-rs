//! A build dependency for running `cmake` to build a native library
//!
//! This crate provides some necessary boilerplate and shim support for running
//! the system `cmake` command to build a native library. It will add
//! appropriate cflags for building code to link into Rust, handle cross
//! compilation, and use the necessary generator for the platform being
//! targeted.
//!
//! The builder-style configuration allows for various variables and such to be
//! passed down into the build as well.
//!
//! ## Installation
//!
//! Add this to your `Cargo.toml`:
//!
//! ```toml
//! [build-dependencies]
//! cmake = "0.1"
//! ```
//!
//! ## Examples
//!
//! ```no_run
//! use cmake;
//!
//! // Builds the project in the directory located in `libfoo`, installing it
//! // into $OUT_DIR
//! let dst = cmake::build("libfoo");
//!
//! println!("cargo:rustc-link-search=native={}", dst.display());
//! println!("cargo:rustc-link-lib=static=foo");
//! ```
//!
//! ```no_run
//! use cmake::Config;
//!
//! let dst = Config::new("libfoo")
//!                  .define("FOO", "BAR")
//!                  .cflag("-foo")
//!                  .build();
//! println!("cargo:rustc-link-search=native={}", dst.display());
//! println!("cargo:rustc-link-lib=static=foo");
//! ```

#![deny(missing_docs)]

extern crate gcc;

use std::env;
use std::ffi::{OsString, OsStr};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Builder style configuration for a pending CMake build.
pub struct Config {
    path: PathBuf,
    cflags: OsString,
    defines: Vec<(OsString, OsString)>,
    deps: Vec<String>,
    target: Option<String>,
    out_dir: Option<PathBuf>,
    profile: Option<String>,
    build_args: Vec<OsString>,
}

/// Builds the native library rooted at `path` with the default cmake options.
/// This will return the directory in which the library was installed.
///
/// # Examples
///
/// ```no_run
/// use cmake;
///
/// // Builds the project in the directory located in `libfoo`, installing it
/// // into $OUT_DIR
/// let dst = cmake::build("libfoo");
///
/// println!("cargo:rustc-link-search=native={}", dst.display());
/// println!("cargo:rustc-link-lib=static=foo");
/// ```
///
pub fn build<P: AsRef<Path>>(path: P) -> PathBuf {
    Config::new(path.as_ref()).build()
}

impl Config {
    /// Creates a new blank set of configuration to build the project specified
    /// at the path `path`.
    pub fn new<P: AsRef<Path>>(path: P) -> Config {
        Config {
            path: path.as_ref().to_path_buf(),
            cflags: OsString::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            profile: None,
            out_dir: None,
            target: None,
            build_args: Vec::new(),
        }
    }

    /// Adds a custom flag to pass down to the compiler, supplementing those
    /// that this library already passes.
    pub fn cflag<P: AsRef<OsStr>>(&mut self, flag: P) -> &mut Config {
        self.cflags.push(" ");
        self.cflags.push(flag.as_ref());
        self
    }

    /// Adds a new `-D` flag to pass to cmake during the generation step.
    pub fn define<K, V>(&mut self, k: K, v: V) -> &mut Config
        where K: AsRef<OsStr>, V: AsRef<OsStr>
    {
        self.defines.push((k.as_ref().to_owned(), v.as_ref().to_owned()));
        self
    }

    /// Registers a dependency for this compilation on the native library built
    /// by Cargo previously.
    ///
    /// This registration will modify the `CMAKE_PREFIX_PATH` environment
    /// variable for the build system generation step.
    pub fn register_dep(&mut self, dep: &str) -> &mut Config {
        self.deps.push(dep.to_string());
        self
    }

    /// Sets the target triple for this compilation.
    ///
    /// This is automatically scraped from `$TARGET` which is set for Cargo
    /// build scripts so it's not necessary to call this from a build script.
    pub fn target(&mut self, target: &str) -> &mut Config {
        self.target = Some(target.to_string());
        self
    }

    /// Sets the output directory for this compilation.
    ///
    /// This is automatically scraped from `$OUT_DIR` which is set for Cargo
    /// build scripts so it's not necessary to call this from a build script.
    pub fn out_dir<P: AsRef<Path>>(&mut self, out: P) -> &mut Config {
        self.out_dir = Some(out.as_ref().to_path_buf());
        self
    }

    /// Sets the profile for this compilation.
    ///
    /// This is automatically scraped from `$PROFILE` which is set for Cargo
    /// build scripts so it's not necessary to call this from a build script.
    pub fn profile(&mut self, profile: &str) -> &mut Config {
        self.profile = Some(profile.to_string());
        self
    }

    /// Add an argument to the final `cmake` build step
    pub fn build_arg<A: AsRef<OsStr>>(&mut self, arg: A) -> &mut Config {
        self.build_args.push(arg.as_ref().to_owned());
        self
    }

    /// Run this configuration, compiling the library with all the configured
    /// options.
    ///
    /// This will run both the build system generator command as well as the
    /// command to build the library.
    pub fn build(&mut self) -> PathBuf {
        let target = self.target.clone().unwrap_or_else(|| {
            env::var("TARGET").unwrap()
        });
        let msvc = target.contains("msvc");
        let compiler = gcc::Config::new().get_compiler();

        let dst = self.out_dir.clone().unwrap_or_else(|| {
            PathBuf::from(&env::var("OUT_DIR").unwrap())
        });
        let _ = fs::create_dir(&dst.join("build"));

        // Add all our dependencies to our cmake paths
        let mut cmake_prefix_path = Vec::new();
        for dep in &self.deps {
            if let Some(root) = env::var_os(&format!("DEP_{}_ROOT", dep)) {
                cmake_prefix_path.push(PathBuf::from(root));
            }
        }
        let system_prefix = env::var_os("CMAKE_PREFIX_PATH")
                                .unwrap_or(OsString::new());
        cmake_prefix_path.extend(env::split_paths(&system_prefix)
                                     .map(|s| s.to_owned()));
        let cmake_prefix_path = env::join_paths(&cmake_prefix_path).unwrap();

        // Build up the first cmake command to build the build system.
        let mut cmd = Command::new("cmake");
        cmd.arg(env::current_dir().unwrap().join(&self.path))
           .current_dir(&dst.join("build"));
        if target.contains("windows-gnu") {
            // On MinGW we need to coerce cmake to not generate a visual studio
            // build system but instead use makefiles that MinGW can use to
            // build.
            cmd.arg("-G").arg("MSYS Makefiles");
        } else if msvc {
            // If we're on MSVC we need to be sure to use the right generator or
            // otherwise we won't get 32/64 bit correct automatically.
            cmd.arg("-G").arg(self.visual_studio_generator(&target));
        }
        let profile = self.profile.clone().unwrap_or_else(|| {
            match &env::var("PROFILE").unwrap()[..] {
                "bench" | "release" => "Release",
                // currently we need to always use the same CRT for MSVC
                _ if msvc => "Release",
                _ => "Debug",
            }.to_string()
        });
        for &(ref k, ref v) in &self.defines {
            let mut os = OsString::from("-D");
            os.push(k);
            os.push("=");
            os.push(v);
            cmd.arg(os);
        }
        let mut dstflag = OsString::from("-DCMAKE_INSTALL_PREFIX=");
        dstflag.push(&dst);

        // Build up the CFLAGS that we're going to use
        let mut cflagsflag = OsString::from("-DCMAKE_C_FLAGS=");
        cflagsflag.push(&self.cflags);
        for arg in compiler.args() {
            cflagsflag.push(" ");
            cflagsflag.push(arg);
        }

        let mut ccompiler = OsString::from("-DCMAKE_C_COMPILER=");
        ccompiler.push(compiler.path());

        run(cmd.arg(&format!("-DCMAKE_BUILD_TYPE={}", profile))
               .arg(dstflag)
               .arg(cflagsflag)
               .arg(ccompiler)
               .env("CMAKE_PREFIX_PATH", cmake_prefix_path), "cmake");

        // And build!
        run(Command::new("cmake")
                    .arg("--build").arg(".")
                    .arg("--target").arg("install")
                    .arg("--config").arg(profile)
                    .arg("--").args(&self.build_args)
                    .current_dir(&dst.join("build")), "cmake");

        println!("cargo:root={}", dst.display());
        return dst
    }

    fn visual_studio_generator(&self, target: &str) -> String {
        // TODO: need a better way of scraping the VS install...
        let candidate = format!("{:?}", gcc::windows_registry::find(target,
                                                                    "cl.exe"));
        let base = if candidate.contains("12.0") {
            "Visual Studio 12 2013"
        } else if candidate.contains("14.0") {
            "Visual Studio 14 2015"
        } else {
            panic!("couldn't determine visual studio generator")
        };

        if target.contains("i686") {
            base.to_string()
        } else if target.contains("x86_64") {
            format!("{} Win64", base)
        } else {
            panic!("unsupported msvc target: {}", target);
        }
    }
}

fn run(cmd: &mut Command, program: &str) {
    println!("running: {:?}", cmd);
    let status = match cmd.status() {
        Ok(status) => status,
        Err(ref e) if e.kind() == ErrorKind::NotFound => {
            fail(&format!("failed to execute command: {}\nis `{}` not installed?",
                          e, program));
        }
        Err(e) => fail(&format!("failed to execute command: {}", e)),
    };
    if !status.success() {
        fail(&format!("command did not execute successfully, got: {}", status));
    }
}

fn fail(s: &str) -> ! {
    panic!("\n{}\n\nbuild script failed, must exit now", s)
}
