//! `update-manifest`: writes, signs and checks the signed list of a release's
//! files (see the library of this crate, and docs/unaligned/UPDATES.md).

use std::{
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context as _, Result, bail, ensure};
use update_manifest::{Manifest, SIGNATURE_FILE_NAME};

/// The private key, when it is not read from a file.
const KEY_VARIABLE: &str = "UPDATE_SIGNING_KEY";

const USAGE: &str = "\
Usage:
  update-manifest new-key <private-key-file>
      Creates a private key in a new file (refused when the file exists) and
      prints the public key that goes with it.

  update-manifest public-key <private-key-file>
      Prints the public key of a private key.

  update-manifest make --product <id> --version <x.y.z> --out <update.json> <file>...
      Writes the manifest of these files: names, sizes, SHA-256.

  update-manifest sign <update.json> [--key-file <private-key-file>]
      Writes <update.json>.sig. Without --key-file the key is read from the
      environment variable UPDATE_SIGNING_KEY.

  update-manifest verify <update.json> <update.json.sig> --public-keys <key>[,<key>]
                         [--version <x.y.z>] [--files <directory>]
      Checks the signature; with --version, that the manifest is this version's;
      with --files, that the files of the directory are the ones the manifest
      describes.
";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("update-manifest: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Command-line arguments: positional ones and `--name value` options.
struct Arguments {
    positional: Vec<String>,
    options: Vec<(String, String)>,
}

impl Arguments {
    fn parse(arguments: Vec<String>, known_options: &[&str]) -> Result<Self> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.strip_prefix("--") {
                Some(name) => {
                    ensure!(known_options.contains(&name), "unknown option --{name}");
                    let value = arguments
                        .next()
                        .with_context(|| format!("--{name} needs a value"))?;
                    options.push((name.to_string(), value));
                }
                None => positional.push(argument),
            }
        }
        Ok(Self {
            positional,
            options,
        })
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(option, _)| option == name)
            .map(|(_, value)| value.as_str())
    }

    fn required(&self, name: &str) -> Result<&str> {
        self.option(name)
            .with_context(|| format!("--{name} is required"))
    }
}

fn run(mut arguments: Vec<String>) -> Result<()> {
    if arguments.is_empty() || arguments[0] == "--help" || arguments[0] == "-h" {
        print!("{USAGE}");
        return Ok(());
    }
    let command = arguments.remove(0);
    match command.as_str() {
        "new-key" => {
            let arguments = Arguments::parse(arguments, &[])?;
            let [key_file] = arguments.positional.as_slice() else {
                bail!("new-key takes one argument: the file to create");
            };
            new_key(Path::new(key_file))
        }
        "public-key" => {
            let arguments = Arguments::parse(arguments, &[])?;
            let [key_file] = arguments.positional.as_slice() else {
                bail!("public-key takes one argument: the private key file");
            };
            println!(
                "{}",
                update_manifest::public_key(&read_key_file(Path::new(key_file))?)?
            );
            Ok(())
        }
        "make" => {
            let arguments = Arguments::parse(arguments, &["product", "version", "out"])?;
            ensure!(
                !arguments.positional.is_empty(),
                "make needs the files of the release"
            );
            let paths: Vec<PathBuf> = arguments.positional.iter().map(PathBuf::from).collect();
            let manifest = Manifest::describe(
                arguments.required("product")?,
                arguments.required("version")?,
                &paths,
            )?;
            let out = arguments.required("out")?;
            fs::write(out, manifest.to_json()?)
                .with_context(|| format!("could not write {out}"))?;
            print_manifest(&manifest);
            println!("written: {out}");
            Ok(())
        }
        "sign" => {
            let arguments = Arguments::parse(arguments, &["key-file"])?;
            let [manifest_file] = arguments.positional.as_slice() else {
                bail!("sign takes one argument: the manifest");
            };
            let private_key = match arguments.option("key-file") {
                Some(key_file) => read_key_file(Path::new(key_file))?,
                None => env::var(KEY_VARIABLE)
                    .ok()
                    .filter(|key| !key.is_empty())
                    .with_context(|| format!("no key: pass --key-file, or set {KEY_VARIABLE}"))?,
            };
            let bytes = fs::read(manifest_file)
                .with_context(|| format!("could not read {manifest_file}"))?;
            let manifest = Manifest::parse(&bytes)?;
            let signature = update_manifest::sign(&bytes, &private_key)?;
            let signature_file = format!("{manifest_file}.sig");
            fs::write(&signature_file, signature)
                .with_context(|| format!("could not write {signature_file}"))?;
            print_manifest(&manifest);
            println!(
                "signed with the key whose public key is {}",
                update_manifest::public_key(&private_key)?
            );
            println!("written: {signature_file}");
            if !signature_file.ends_with(SIGNATURE_FILE_NAME) {
                println!("note: in a release the file must be named {SIGNATURE_FILE_NAME}");
            }
            Ok(())
        }
        "verify" => {
            let arguments = Arguments::parse(arguments, &["public-keys", "version", "files"])?;
            let [manifest_file, signature_file] = arguments.positional.as_slice() else {
                bail!("verify takes two arguments: the manifest and its signature");
            };
            let bytes = fs::read(manifest_file)
                .with_context(|| format!("could not read {manifest_file}"))?;
            let signature = fs::read_to_string(signature_file)
                .with_context(|| format!("could not read {signature_file}"))?;
            let public_keys =
                update_manifest::parse_public_keys(arguments.required("public-keys")?)?;
            let manifest = update_manifest::verify(&bytes, &signature, &public_keys)?;
            print_manifest(&manifest);
            println!("signature: valid");
            if let Some(version) = arguments.option("version") {
                ensure!(
                    manifest.version == version,
                    "the manifest is version {}, not {version}",
                    manifest.version
                );
            }
            if let Some(directory) = arguments.option("files") {
                let absent = update_manifest::check_files(&manifest, Path::new(directory))?;
                ensure!(
                    absent.is_empty(),
                    "listed in the manifest but not in {directory}: {}",
                    absent.join(", ")
                );
                println!("files: the ones the manifest describes");
            }
            Ok(())
        }
        other => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn print_manifest(manifest: &Manifest) {
    println!("{} {}", manifest.product, manifest.version);
    for file in &manifest.files {
        println!("  {}  {:>11} bytes  {}", file.sha256, file.size, file.name);
    }
}

fn read_key_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("could not read the key file {path:?}"))
}

fn new_key(path: &Path) -> Result<()> {
    let private_key = update_manifest::generate_private_key()?;
    let public_key = update_manifest::public_key(&private_key)?;

    let mut options = fs::OpenOptions::new();
    // Never replaces a key: losing one cuts every installed copy off from
    // updates signed with it.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!("could not create {path:?} (an existing file is never replaced)")
    })?;
    writeln!(file, "{private_key}")?;

    println!("private key written to {}", path.display());
    println!("public key: {public_key}");
    Ok(())
}
