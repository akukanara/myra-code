use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum ShellType {
    Zsh,
    Bash,
    PowerShell,
    Sh,
    Cmd,
}

impl ShellType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::PowerShell => "powershell",
            Self::Sh => "sh",
            Self::Cmd => "cmd",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedShell {
    pub shell_type: ShellType,
    pub shell_path: PathBuf,
}

impl DetectedShell {
    pub fn name(&self) -> &'static str {
        self.shell_type.name()
    }
}

pub fn detect_shell_type(shell_path: impl AsRef<std::path::Path>) -> Option<ShellType> {
    let shell_path = shell_path.as_ref();
    match shell_path.as_os_str().to_str() {
        Some("zsh") => Some(ShellType::Zsh),
        Some("sh") => Some(ShellType::Sh),
        Some("cmd") => Some(ShellType::Cmd),
        Some("bash") => Some(ShellType::Bash),
        Some("pwsh") => Some(ShellType::PowerShell),
        Some("powershell") => Some(ShellType::PowerShell),
        _ => {
            let shell_name = shell_path.file_stem();
            if let Some(shell_name) = shell_name {
                let shell_name_path = std::path::Path::new(shell_name);
                if shell_name_path != shell_path {
                    return detect_shell_type(shell_name_path);
                }
            }
            None
        }
    }
}

#[cfg(unix)]
fn get_user_shell_path() -> Option<PathBuf> {
    let uid = unsafe { libc::getuid() };
    use std::ffi::CStr;
    use std::mem::MaybeUninit;
    use std::ptr;

    let mut passwd = MaybeUninit::<libc::passwd>::uninit();

    // We cannot use getpwuid here: it returns pointers into libc-managed
    // storage, which is not safe to read concurrently on all targets (the musl
    // static build used by the CLI can segfault when parallel callers race on
    // that buffer). getpwuid_r keeps the passwd data in caller-owned memory.
    let suggested_buffer_len = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let buffer_len = usize::try_from(suggested_buffer_len)
        .ok()
        .filter(|len| *len > 0)
        .unwrap_or(1024);
    let mut buffer = vec![0; buffer_len];

    loop {
        let mut result = ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };

        if status == 0 {
            if result.is_null() {
                return None;
            }

            let passwd = unsafe { passwd.assume_init_ref() };
            if passwd.pw_shell.is_null() {
                return None;
            }

            let shell_path = unsafe { CStr::from_ptr(passwd.pw_shell) }
                .to_string_lossy()
                .into_owned();
            return Some(PathBuf::from(shell_path));
        }

        if status != libc::ERANGE {
            return None;
        }

        // Retry with a larger buffer until libc can materialize the passwd entry.
        let new_len = buffer.len().checked_mul(2)?;
        if new_len > 1024 * 1024 {
            return None;
        }
        buffer.resize(new_len, 0);
    }
}

#[cfg(not(unix))]
fn get_user_shell_path() -> Option<PathBuf> {
    None
}

fn file_exists(path: &std::path::Path) -> Option<PathBuf> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        Some(PathBuf::from(path))
    } else {
        None
    }
}

/// A resolved interpreter, split by whether it can actually be launched under the Windows sandbox.
struct ShellPathCandidates {
    /// A real executable on disk.
    real: Option<PathBuf>,
    /// A Windows app execution alias: fine from a normal shell, never from the sandbox.
    alias: Option<PathBuf>,
}

impl ShellPathCandidates {
    fn preferred(self) -> Option<PathBuf> {
        self.real.or(self.alias)
    }
}

/// Whether a path is a Windows app execution alias rather than a real interpreter.
///
/// `%LOCALAPPDATA%\Microsoft\WindowsApps` holds zero-byte reparse points that only start their
/// target through AppX activation, and `C:\Program Files\WindowsApps` -- where the package itself
/// lives -- is readable only under the package identity. Neither is reachable from the restricted
/// token the Windows sandbox spawns with, so `CreateProcessAsUserW` on one fails with
/// ERROR_ACCESS_DENIED every time, whatever the command was. `WindowsApps` is on PATH by default,
/// so a Store-installed PowerShell wins the `which` lookup and would otherwise make every
/// sandboxed command unrunnable. Keep such a hit as a last resort, behind any real install.
fn is_app_execution_alias(path: &std::path::Path) -> bool {
    // Split on both separators rather than using `Path::components`: on a non-Windows host a
    // Windows path is a single component, which would silently make this always false and leave the
    // behaviour untestable off Windows.
    path.to_string_lossy()
        .split(['\\', '/'])
        .any(|segment| segment.eq_ignore_ascii_case("WindowsApps"))
}

fn get_shell_path_candidates(
    shell_type: ShellType,
    provided_path: Option<&PathBuf>,
    binary_name: &str,
    fallback_paths: &[&str],
) -> ShellPathCandidates {
    // An explicitly configured path is taken as-is: the user asked for that interpreter, and it is
    // not this function's place to second-guess it.
    if let Some(path) = provided_path.and_then(|path| file_exists(path)) {
        return ShellPathCandidates {
            real: Some(path),
            alias: None,
        };
    }

    let default_shell_path = get_user_shell_path();
    if let Some(default_shell_path) = default_shell_path
        && detect_shell_type(&default_shell_path) == Some(shell_type)
        && file_exists(&default_shell_path).is_some()
    {
        return ShellPathCandidates {
            real: Some(default_shell_path),
            alias: None,
        };
    }

    let mut alias = None;
    match which::which(binary_name) {
        Ok(path) if is_app_execution_alias(&path) => alias = Some(path),
        Ok(path) => {
            return ShellPathCandidates {
                real: Some(path),
                alias: None,
            };
        }
        Err(_) => {}
    }

    for path in fallback_paths {
        if let Some(path) = file_exists(std::path::Path::new(path)) {
            return ShellPathCandidates {
                real: Some(path),
                alias,
            };
        }
    }

    ShellPathCandidates { real: None, alias }
}

fn get_shell_path(
    shell_type: ShellType,
    provided_path: Option<&PathBuf>,
    binary_name: &str,
    fallback_paths: &[&str],
) -> Option<PathBuf> {
    get_shell_path_candidates(shell_type, provided_path, binary_name, fallback_paths).preferred()
}

const ZSH_FALLBACK_PATHS: &[&str] = &["/bin/zsh"];

fn get_zsh_shell(path: Option<&PathBuf>) -> Option<DetectedShell> {
    let shell_path = get_shell_path(ShellType::Zsh, path, "zsh", ZSH_FALLBACK_PATHS);

    shell_path.map(|shell_path| DetectedShell {
        shell_type: ShellType::Zsh,
        shell_path,
    })
}

const BASH_FALLBACK_PATHS: &[&str] = &["/bin/bash", "/usr/bin/bash"];

fn get_bash_shell(path: Option<&PathBuf>) -> Option<DetectedShell> {
    let shell_path = get_shell_path(ShellType::Bash, path, "bash", BASH_FALLBACK_PATHS);

    shell_path.map(|shell_path| DetectedShell {
        shell_type: ShellType::Bash,
        shell_path,
    })
}

const SH_FALLBACK_PATHS: &[&str] = &["/bin/sh"];

fn get_sh_shell(path: Option<&PathBuf>) -> Option<DetectedShell> {
    let shell_path = get_shell_path(ShellType::Sh, path, "sh", SH_FALLBACK_PATHS);

    shell_path.map(|shell_path| DetectedShell {
        shell_type: ShellType::Sh,
        shell_path,
    })
}

// Note the `pwsh` and `powershell` fallback paths are where the respective
// shells are commonly installed on GitHub Actions Windows runners, but may not
// be present on all Windows machines:
// https://docs.github.com/en/actions/tutorials/build-and-test-code/powershell

#[cfg(windows)]
const PWSH_FALLBACK_PATHS: &[&str] = &[r#"C:\Program Files\PowerShell\7\pwsh.exe"#];
#[cfg(not(windows))]
const PWSH_FALLBACK_PATHS: &[&str] = &["/usr/local/bin/pwsh"];

#[cfg(windows)]
const POWERSHELL_FALLBACK_PATHS: &[&str] =
    &[r#"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"#];
#[cfg(not(windows))]
const POWERSHELL_FALLBACK_PATHS: &[&str] = &[];

/// Pick between the two PowerShell flavours, real installs first.
///
/// Keeps the usual pwsh-before-powershell preference, but falling all the way through to System32's
/// powershell.exe beats returning a Store alias the sandbox cannot launch. An alias is only worth
/// handing back when no real interpreter exists at all.
fn prefer_real_interpreter(
    pwsh: ShellPathCandidates,
    powershell: ShellPathCandidates,
) -> Option<PathBuf> {
    pwsh.real
        .or(powershell.real)
        .or(pwsh.alias)
        .or(powershell.alias)
}

fn get_powershell_shell(path: Option<&PathBuf>) -> Option<DetectedShell> {
    let pwsh = get_shell_path_candidates(ShellType::PowerShell, path, "pwsh", PWSH_FALLBACK_PATHS);
    let powershell = get_shell_path_candidates(
        ShellType::PowerShell,
        path,
        "powershell",
        POWERSHELL_FALLBACK_PATHS,
    );

    let shell_path = prefer_real_interpreter(pwsh, powershell);

    shell_path.map(|shell_path| DetectedShell {
        shell_type: ShellType::PowerShell,
        shell_path,
    })
}

fn get_cmd_shell(path: Option<&PathBuf>) -> Option<DetectedShell> {
    let shell_path = get_shell_path(ShellType::Cmd, path, "cmd", &[]);

    shell_path.map(|shell_path| DetectedShell {
        shell_type: ShellType::Cmd,
        shell_path,
    })
}

pub fn ultimate_fallback_shell() -> DetectedShell {
    if cfg!(windows) {
        DetectedShell {
            shell_type: ShellType::Cmd,
            shell_path: PathBuf::from("cmd.exe"),
        }
    } else {
        DetectedShell {
            shell_type: ShellType::Sh,
            shell_path: PathBuf::from("/bin/sh"),
        }
    }
}

pub fn get_shell_by_model_provided_path(shell_path: &PathBuf) -> DetectedShell {
    detect_shell_type(shell_path)
        .and_then(|shell_type| get_shell(shell_type, Some(shell_path)))
        .unwrap_or_else(ultimate_fallback_shell)
}

pub fn get_shell(shell_type: ShellType, path: Option<&PathBuf>) -> Option<DetectedShell> {
    match shell_type {
        ShellType::Zsh => get_zsh_shell(path),
        ShellType::Bash => get_bash_shell(path),
        ShellType::PowerShell => get_powershell_shell(path),
        ShellType::Sh => get_sh_shell(path),
        ShellType::Cmd => get_cmd_shell(path),
    }
}

pub fn default_user_shell() -> DetectedShell {
    default_user_shell_from_path(get_user_shell_path())
}

pub fn default_user_shell_from_path(user_shell_path: Option<PathBuf>) -> DetectedShell {
    if cfg!(windows) {
        get_shell(ShellType::PowerShell, /*path*/ None).unwrap_or_else(ultimate_fallback_shell)
    } else {
        let user_default_shell = user_shell_path
            .and_then(|shell| detect_shell_type(&shell))
            .and_then(|shell_type| get_shell(shell_type, /*path*/ None));

        let shell_with_fallback = if cfg!(target_os = "macos") {
            user_default_shell
                .or_else(|| get_shell(ShellType::Zsh, /*path*/ None))
                .or_else(|| get_shell(ShellType::Bash, /*path*/ None))
        } else {
            user_default_shell
                .or_else(|| get_shell(ShellType::Bash, /*path*/ None))
                .or_else(|| get_shell(ShellType::Zsh, /*path*/ None))
        };

        shell_with_fallback.unwrap_or_else(ultimate_fallback_shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn real(path: &str) -> ShellPathCandidates {
        ShellPathCandidates {
            real: Some(PathBuf::from(path)),
            alias: None,
        }
    }

    fn alias(path: &str) -> ShellPathCandidates {
        ShellPathCandidates {
            real: None,
            alias: Some(PathBuf::from(path)),
        }
    }

    fn none() -> ShellPathCandidates {
        ShellPathCandidates {
            real: None,
            alias: None,
        }
    }

    #[test]
    fn recognizes_app_execution_aliases() {
        for path in [
            r"C:\Users\A Y\AppData\Local\Microsoft\WindowsApps\pwsh.exe",
            r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.3.0_x64__8wekyb3d8bbwe\pwsh.exe",
            r"c:\users\a y\appdata\local\microsoft\windowsapps\pwsh.exe",
        ] {
            assert!(is_app_execution_alias(std::path::Path::new(path)), "{path}");
        }

        for path in [
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            "/usr/local/bin/pwsh",
        ] {
            assert!(
                !is_app_execution_alias(std::path::Path::new(path)),
                "{path}"
            );
        }
    }

    #[test]
    fn prefers_real_pwsh_over_everything() {
        assert_eq!(
            prefer_real_interpreter(
                real(r"C:\Program Files\PowerShell\7\pwsh.exe"),
                real(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            ),
            Some(PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"))
        );
    }

    #[test]
    fn falls_through_alias_pwsh_to_real_powershell() {
        // The regression this guards: a Store-installed pwsh puts an app execution alias on PATH,
        // which `which` finds first. Spawning it under the sandbox's restricted token always fails
        // with ERROR_ACCESS_DENIED, so System32's powershell.exe has to win instead.
        assert_eq!(
            prefer_real_interpreter(
                alias(r"C:\Users\A Y\AppData\Local\Microsoft\WindowsApps\pwsh.exe"),
                real(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            ),
            Some(PathBuf::from(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
            ))
        );
    }

    #[test]
    fn returns_alias_only_as_a_last_resort() {
        // Better to run the alias -- which works fine outside the sandbox -- than to report no
        // PowerShell at all and fall back to cmd.exe.
        let alias_path = r"C:\Users\A Y\AppData\Local\Microsoft\WindowsApps\pwsh.exe";
        assert_eq!(
            prefer_real_interpreter(alias(alias_path), none()),
            Some(PathBuf::from(alias_path))
        );
        assert_eq!(prefer_real_interpreter(none(), none()), None);
    }

    #[test]
    fn test_detect_shell_type() {
        assert_eq!(
            detect_shell_type(PathBuf::from("zsh")),
            Some(ShellType::Zsh)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("bash")),
            Some(ShellType::Bash)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("pwsh")),
            Some(ShellType::PowerShell)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("powershell")),
            Some(ShellType::PowerShell)
        );
        assert_eq!(detect_shell_type(PathBuf::from("fish")), None);
        assert_eq!(detect_shell_type(PathBuf::from("other")), None);
        assert_eq!(
            detect_shell_type(PathBuf::from("/bin/zsh")),
            Some(ShellType::Zsh)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("/bin/bash")),
            Some(ShellType::Bash)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("/usr/bin/bash")),
            Some(ShellType::Bash)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("powershell.exe")),
            Some(ShellType::PowerShell)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from(if cfg!(windows) {
                "C:\\windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
            } else {
                "/usr/local/bin/pwsh"
            })),
            Some(ShellType::PowerShell)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("pwsh.exe")),
            Some(ShellType::PowerShell)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("/usr/local/bin/pwsh")),
            Some(ShellType::PowerShell)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("/bin/sh")),
            Some(ShellType::Sh)
        );
        assert_eq!(detect_shell_type(PathBuf::from("sh")), Some(ShellType::Sh));
        assert_eq!(
            detect_shell_type(PathBuf::from("cmd")),
            Some(ShellType::Cmd)
        );
        assert_eq!(
            detect_shell_type(PathBuf::from("cmd.exe")),
            Some(ShellType::Cmd)
        );
    }
}
