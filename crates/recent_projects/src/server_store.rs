//! What the Servers panel and its form keep about a server, beyond the
//! `ssh_connections` entry of the settings:
//!
//! - **the key**: the entry's `args` name it, as `ssh` wants it (`-i <file>
//!   -o IdentitiesOnly=yes`). A key the person pasted, or one that had to be
//!   converted or copied, is a file of the application's own, in
//!   `<config>/ssh_keys/`; a key that already sits in `~/.ssh` is used where
//!   it is;
//! - **the password or the key's passphrase**, when the person typed one: in
//!   the system keychain, never in the settings file.

use std::path::{Path, PathBuf};

use gpui::AsyncApp;
use settings::SshConnection;
use util::ResultExt as _;

/// `ssh` tries every key of the agent before the one it is given, and a
/// server closes the connection after a few refused keys ("Too many
/// authentication failures"). With this option it offers the given key only.
const ONLY_THIS_KEY: &str = "IdentitiesOnly=yes";

/// The folder of the key files the application made itself.
pub(crate) fn managed_keys_dir() -> PathBuf {
    paths::config_dir().join("ssh_keys")
}

/// Whether `path` is one of the application's own key files.
pub(crate) fn is_managed_key(path: &Path) -> bool {
    path.parent() == Some(managed_keys_dir().as_path())
}

/// The key file the arguments of a connection name, if any.
pub(crate) fn identity_file(args: &[String]) -> Option<&str> {
    let position = args.iter().position(|arg| arg == "-i")?;
    args.get(position + 1).map(String::as_str)
}

/// The arguments without a key: for signing in with a password, and before
/// another key is set. Arguments this module did not write are kept.
pub(crate) fn without_identity(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let is_pair = |flag: &str, value: Option<&str>| {
            args[index] == flag
                && args
                    .get(index + 1)
                    .is_some_and(|next| value.is_none_or(|value| next == value))
        };
        if is_pair("-i", None) || is_pair("-o", Some(ONLY_THIS_KEY)) {
            index += 2;
        } else {
            kept.push(args[index].clone());
            index += 1;
        }
    }
    kept
}

/// The arguments with `key` as the one key to sign in with.
pub(crate) fn with_identity(args: &[String], key: &Path) -> Vec<String> {
    let mut args = without_identity(args);
    args.extend([
        "-i".to_string(),
        key.to_string_lossy().into_owned(),
        "-o".to_string(),
        ONLY_THIS_KEY.to_string(),
    ]);
    args
}

/// Removes a key file of the application's own once no server uses it any
/// more. A key that is not the application's is never touched.
pub(crate) fn remove_key_if_unused(key: &Path, servers: &[SshConnection]) {
    if !is_managed_key(key) {
        return;
    }
    let in_use = servers
        .iter()
        .any(|server| identity_file(&server.args).is_some_and(|used| Path::new(used) == key));
    if !in_use {
        std::fs::remove_file(key).log_err();
    }
}

/// The name under which the secret of a server is kept in the keychain.
pub(crate) fn secret_name(host: &str, username: Option<&str>, port: Option<u16>) -> String {
    let mut name = String::from("ssh://");
    if let Some(username) = username {
        name.push_str(username);
        name.push('@');
    }
    name.push_str(host);
    if let Some(port) = port {
        name.push(':');
        name.push_str(&port.to_string());
    }
    name
}

pub(crate) fn secret_name_of(server: &SshConnection) -> String {
    secret_name(&server.host, server.username.as_deref(), server.port)
}

/// The saved password or passphrase of a server, if there is one.
pub(crate) async fn read_secret(name: &str, cx: &AsyncApp) -> Option<String> {
    let provider = cx.update(|cx| app_credentials_provider::global(cx));
    let (_, secret) = provider.read_credentials(name, cx).await.log_err()??;
    String::from_utf8(secret).ok()
}

pub(crate) async fn write_secret(name: &str, secret: &str, cx: &AsyncApp) -> anyhow::Result<()> {
    let provider = cx.update(|cx| app_credentials_provider::global(cx));
    provider
        .write_credentials(name, "ssh", secret.as_bytes(), cx)
        .await
}

pub(crate) async fn delete_secret(name: &str, cx: &AsyncApp) {
    let provider = cx.update(|cx| app_credentials_provider::global(cx));
    // Nothing saved under that name is not an error worth a line in the log.
    provider.delete_credentials(name, cx).await.ok();
}

/// How a server is called in lists: its name, or its address.
pub(crate) fn display_name(server: &SshConnection) -> String {
    server
        .nickname
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| server.host.clone())
}

/// `user@host`, with the port when it is not 22.
pub(crate) fn address(server: &SshConnection) -> String {
    let mut address = String::new();
    if let Some(username) = &server.username {
        address.push_str(username);
        address.push('@');
    }
    address.push_str(&server.host);
    if let Some(port) = server.port.filter(|port| *port != 22) {
        address.push(':');
        address.push_str(&port.to_string());
    }
    address
}

/// What a person typed in the address field, taken apart. People paste what
/// they were given: `203.0.113.4`, `root@203.0.113.4`, `root@host:2222`, or
/// the whole command `ssh root@host -p 2222`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TypedAddress {
    pub(crate) host: String,
    pub(crate) username: Option<String>,
    pub(crate) port: Option<u16>,
}

pub(crate) fn parse_address(typed: &str) -> Result<TypedAddress, String> {
    let typed = typed.trim();
    let typed = typed.strip_prefix("ssh://").unwrap_or(typed);
    let mut words = typed.split_whitespace().peekable();
    if words.peek() == Some(&"ssh") {
        words.next();
    }

    let mut target = None;
    let mut port = None;
    while let Some(word) = words.next() {
        if word == "-p" {
            let value = words.next().ok_or("A port number is missing after -p.")?;
            port = Some(parse_port(value)?);
        } else if let Some(value) = word.strip_prefix("-p").filter(|value| !value.is_empty()) {
            port = Some(parse_port(value)?);
        } else if word.starts_with('-') {
            return Err(format!(
                "Write the address only, without the option \"{word}\"."
            ));
        } else if target.is_none() {
            target = Some(word);
        } else {
            return Err("Write one address only, without spaces.".to_string());
        }
    }
    let target = target.ok_or("Write the address of the server.")?;

    let (username, host) = match target.rsplit_once('@') {
        Some((username, host)) => (Some(username), host),
        None => (None, target),
    };
    // `host:2222`, but not an IPv6 address, which is all colons.
    let (host, port_of_host) = match host.rsplit_once(':') {
        Some((name, digits)) if !name.contains(':') && !digits.is_empty() => {
            (name, Some(parse_port(digits)?))
        }
        _ => (host, None),
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.is_empty() {
        return Err("Write the address of the server.".to_string());
    }
    if username.is_some_and(str::is_empty) {
        return Err("The user name before \"@\" is missing.".to_string());
    }
    Ok(TypedAddress {
        host: host.to_string(),
        username: username.map(str::to_string),
        port: port.or(port_of_host),
    })
}

pub(crate) fn parse_port(typed: &str) -> Result<u16, String> {
    match typed.trim().parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(format!(
            "\"{}\" is not a port: a number from 1 to 65535.",
            typed.trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn test_the_key_is_set_replaced_and_removed_without_touching_other_arguments() {
        let plain = args(&["-o", "ServerAliveInterval=30", "-J", "jump"]);
        assert_eq!(identity_file(&plain), None);

        let with_key = with_identity(&plain, Path::new("/keys/one"));
        assert_eq!(
            with_key,
            args(&[
                "-o",
                "ServerAliveInterval=30",
                "-J",
                "jump",
                "-i",
                "/keys/one",
                "-o",
                "IdentitiesOnly=yes"
            ])
        );
        assert_eq!(identity_file(&with_key), Some("/keys/one"));

        let replaced = with_identity(&with_key, Path::new("/keys/two"));
        assert_eq!(identity_file(&replaced), Some("/keys/two"));
        assert_eq!(replaced.iter().filter(|arg| *arg == "-i").count(), 1);
        assert_eq!(
            replaced
                .iter()
                .filter(|arg| *arg == "IdentitiesOnly=yes")
                .count(),
            1
        );

        assert_eq!(without_identity(&replaced), plain);
        // A lone `-i` at the end is not a pair: it is left for ssh to refuse.
        assert_eq!(without_identity(&args(&["-i"])), args(&["-i"]));
    }

    #[test]
    fn test_what_people_type_as_an_address() {
        let typed = |host: &str, username: Option<&str>, port: Option<u16>| TypedAddress {
            host: host.to_string(),
            username: username.map(str::to_string),
            port,
        };
        assert_eq!(
            parse_address("203.0.113.4"),
            Ok(typed("203.0.113.4", None, None))
        );
        assert_eq!(
            parse_address("  root@example.org "),
            Ok(typed("example.org", Some("root"), None))
        );
        assert_eq!(
            parse_address("deploy@example.org:2222"),
            Ok(typed("example.org", Some("deploy"), Some(2222)))
        );
        assert_eq!(
            parse_address("ssh root@example.org -p 2222"),
            Ok(typed("example.org", Some("root"), Some(2222)))
        );
        assert_eq!(
            parse_address("ssh -p2222 root@example.org"),
            Ok(typed("example.org", Some("root"), Some(2222)))
        );
        assert_eq!(
            parse_address("ssh://root@example.org:22"),
            Ok(typed("example.org", Some("root"), Some(22)))
        );
        // An IPv6 address is not a host and a port.
        assert_eq!(
            parse_address("root@2001:db8::1"),
            Ok(typed("2001:db8::1", Some("root"), None))
        );
        assert_eq!(
            parse_address("[2001:db8::1]"),
            Ok(typed("2001:db8::1", None, None))
        );

        assert!(parse_address("").is_err());
        assert!(parse_address("ssh").is_err());
        assert!(parse_address("@example.org").is_err());
        assert!(parse_address("example.org:0").is_err());
        assert!(parse_address("example.org:70000").is_err());
        assert!(parse_address("one two").is_err());
        assert!(parse_address("ssh -i key root@example.org").is_err());
    }

    #[test]
    fn test_names_shown_and_kept() {
        let mut server = SshConnection {
            host: "example.org".to_string(),
            username: Some("root".to_string()),
            port: Some(22),
            ..Default::default()
        };
        assert_eq!(display_name(&server), "example.org");
        assert_eq!(address(&server), "root@example.org");
        assert_eq!(secret_name_of(&server), "ssh://root@example.org:22");

        server.nickname = Some("Production".to_string());
        server.port = Some(2222);
        assert_eq!(display_name(&server), "Production");
        assert_eq!(address(&server), "root@example.org:2222");

        server.username = None;
        server.port = None;
        server.nickname = Some("  ".to_string());
        assert_eq!(display_name(&server), "example.org");
        assert_eq!(address(&server), "example.org");
        assert_eq!(secret_name_of(&server), "ssh://example.org");
    }
}
