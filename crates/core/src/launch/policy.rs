//! JVM 和游戏参数的边界。只有版本描述可以设置加载器自己的类路径。

use crate::config::EnvVar;

pub(super) const JAVA_OPTION_ENV: &[&str] = &[
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "IBM_JAVA_OPTIONS",
    "OPENJ9_JAVA_OPTIONS",
];

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum LaunchParameterError {
    #[error("自定义游戏参数不能修改登录账户或游戏会话，请检查此实例的启动设置")]
    CustomGame,
    #[error("自定义 JVM 参数会加载代理或改写启动方式，请检查此实例的启动设置")]
    CustomJvm,
    #[error("启动环境变量不能注入 JVM 参数，请检查此实例的启动设置")]
    Environment,
    #[error("此实例的版本参数与 Microsoft 登录不兼容，请检查版本或加载器")]
    Version,
}

const GAME_IDENTITY: &[(&str, &str)] = &[
    ("--username", "${auth_player_name}"),
    ("--uuid", "${auth_uuid}"),
    ("--accessToken", "${auth_access_token}"),
    ("--session", "${auth_session}"),
    ("--userType", "${user_type}"),
    ("--userProperties", "${user_properties}"),
    ("--profileProperties", "${profile_properties}"),
    ("--xuid", "${auth_xuid}"),
    ("--clientId", "${clientid}"),
];

pub(super) fn protected_game_arg(arg: &str) -> bool {
    let start = arg.trim_start();
    if !start.starts_with('-') {
        return false;
    }
    let name = start
        .trim_start_matches('-')
        .split(|c: char| c == '=' || c.is_whitespace())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    // 一些旧版游戏允许缩写，因此也阻止认证选项的缩写和单横线写法。
    name.is_empty()
        || "demo".starts_with(&name)
        || GAME_IDENTITY
            .iter()
            .any(|(key, _)| key[2..].to_ascii_lowercase().starts_with(&name))
}

pub(super) fn validate_custom_game(args: &[String]) -> Result<(), LaunchParameterError> {
    if args.iter().any(|arg| protected_game_arg(arg)) {
        Err(LaunchParameterError::CustomGame)
    } else {
        Ok(())
    }
}

pub(super) fn validate_version_game(args: &[&str]) -> Result<(), LaunchParameterError> {
    let mut seen = std::collections::HashSet::new();
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        if !protected_game_arg(arg) {
            if contains_session_placeholder(arg) {
                return Err(LaunchParameterError::Version);
            }
            continue;
        }
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (arg, None),
        };
        let Some((_, expected)) = GAME_IDENTITY.iter().find(|(flag, _)| *flag == key) else {
            return Err(LaunchParameterError::Version);
        };
        if !seen.insert(key) || inline.or_else(|| args.next()) != Some(*expected) {
            return Err(LaunchParameterError::Version);
        }
    }

    let valid = seen.contains("--username")
        && (seen.contains("--session")
            || (seen.contains("--accessToken") && seen.contains("--uuid")));
    if valid {
        Ok(())
    } else {
        Err(LaunchParameterError::Version)
    }
}

pub(super) fn validate_env(vars: &[EnvVar]) -> Result<(), LaunchParameterError> {
    if vars.iter().any(|var| {
        JAVA_OPTION_ENV
            .iter()
            .any(|key| var.name.trim().eq_ignore_ascii_case(key))
    }) {
        Err(LaunchParameterError::Environment)
    } else {
        Ok(())
    }
}

pub(super) fn validate_custom_jvm(args: &[String]) -> Result<(), LaunchParameterError> {
    validate_jvm(args.iter().map(String::as_str), false)
        .then_some(())
        .ok_or(LaunchParameterError::CustomJvm)
}

pub(super) fn validate_version_jvm(
    args: impl Iterator<Item = impl AsRef<str>>,
) -> Result<(), LaunchParameterError> {
    validate_jvm(args, true)
        .then_some(())
        .ok_or(LaunchParameterError::Version)
}

fn validate_jvm(args: impl Iterator<Item = impl AsRef<str>>, version: bool) -> bool {
    let mut args = args;
    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        if contains_session_placeholder(arg) {
            return false;
        }
        if let Some(prop) = arg.strip_prefix("-D") {
            if !safe_system_property(prop, version) {
                return false;
            }
            continue;
        }
        if let Some(flag) = arg.strip_prefix("-XX:") {
            let key = flag
                .trim_start_matches(['+', '-'])
                .split('=')
                .next()
                .unwrap_or_default();
            if matches!(
                key,
                "" | "Flags" | "VMOptionsFile" | "OnError" | "OnOutOfMemoryError"
            ) {
                return false;
            }
            continue;
        }
        if arg.starts_with("-X")
            && !arg.starts_with("-Xbootclasspath")
            && !arg.starts_with("-Xrun")
            && !arg.starts_with("-Xoptionsfile")
            && arg != "-Xdebug"
        {
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg, None),
        };
        match flag {
            "--add-modules"
            | "--add-exports"
            | "--add-opens"
            | "--add-reads"
            | "--enable-native-access"
            | "--limit-modules" => {}
            "-cp" | "-classpath" | "--class-path" | "-p" | "--module-path" if version => {}
            "--enable-preview" | "-ea" | "-da" | "-esa" | "-dsa" | "-enableassertions"
            | "-disableassertions" | "-verbose" | "-server" | "-client"
                if inline.is_none() =>
            {
                continue;
            }
            _ if arg.starts_with("-ea:")
                || arg.starts_with("-da:")
                || arg.starts_with("-verbose:") =>
            {
                continue;
            }
            _ => return false,
        }
        let valid_value = |value: &str| {
            if value.trim().is_empty()
                || value.starts_with(['-', '@'])
                || contains_session_placeholder(value)
            {
                return false;
            }
            match flag {
                "-cp" | "-classpath" | "--class-path" => value == "${classpath}",
                "-p" | "--module-path" => value.split("${classpath_separator}").all(|entry| {
                    entry
                        .strip_prefix("${library_directory}/")
                        .is_some_and(|path| {
                            !path.contains(['\\', ';', ':'])
                                && !path.contains("${")
                                && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                        })
                }),
                _ => true,
            }
        };
        if !inline.map_or_else(
            || args.next().is_some_and(|arg| valid_value(arg.as_ref())),
            valid_value,
        ) {
            return false;
        }
    }
    true
}

fn contains_session_placeholder(arg: &str) -> bool {
    ["${auth_access_token}", "${auth_session}"]
        .iter()
        .any(|placeholder| arg.contains(placeholder))
}

fn safe_system_property(property: &str, version: bool) -> bool {
    if property
        .split('=')
        .next()
        .is_some_and(|key| key.eq_ignore_ascii_case("libraryDirectory"))
    {
        return version && property == "libraryDirectory=${library_directory}";
    }
    let key = property
        .split_once('=')
        .map_or(property, |(key, _)| key)
        .to_ascii_lowercase();
    !key.is_empty()
        && !key.contains("${")
        && ![
            "authlib",
            "yggdrasil",
            "minecraft.api.",
            "minecraft.auth.",
            "minecraft.session.",
            "minecraft.authlib.",
            "jdk.module.",
        ]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        && !matches!(
            key.as_str(),
            "java.system.class.loader"
                | "java.class.path"
                | "sun.boot.class.path"
                | "java.security.properties"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_game_flags_cannot_override_the_verified_session() {
        for arg in [
            "--username=Other",
            "-uuid",
            "--accessT",
            "--userType",
            "--session",
            "--profileProperties",
            "--clientId",
            "--xuid",
            "--dem",
            "--",
        ] {
            assert_eq!(
                validate_custom_game(&[arg.to_owned()]),
                Err(LaunchParameterError::CustomGame),
                "{arg}"
            );
        }
        assert!(validate_custom_game(&["--fullscreen".to_owned()]).is_ok());
    }

    #[test]
    fn custom_jvm_cannot_start_a_different_program_or_redirect_auth() {
        for args in [
            &["-javaagent:custom.jar"][..],
            &["-agentpath:custom.dll"],
            &["-Dminecraft.api.env=custom"],
            &["-Dauthlibinjector.yggdrasil.prefetched=data"],
            &["-Xbootclasspath/a:custom.jar"],
            &["--patch-module=minecraft=custom.jar"],
            &["-XX:VMOptionsFile=opts.txt"],
            &["@opts.txt"],
            &["-jar", "custom.jar"],
            &["com.example.Main"],
            &["-cp", "custom.jar"],
            &["--add-opens", "@opts.txt"],
        ] {
            let args: Vec<_> = args.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(
                validate_custom_jvm(&args),
                Err(LaunchParameterError::CustomJvm)
            );
        }
        let args = [
            "-Xss4M",
            "-XX:+UseZGC",
            "-Dcustom.key=custom value",
            "--add-opens",
            "java.base/java.io=ALL-UNNAMED",
        ]
        .map(str::to_owned);
        assert!(validate_custom_jvm(&args).is_ok());
    }

    #[test]
    fn java_option_environment_keys_use_windows_case_insensitive_names() {
        for key in ["java_tool_options", "_Java_Options", "JDK_JAVA_OPTIONS"] {
            let env = [EnvVar {
                name: key.to_owned(),
                value: "-Dauthlib=example".to_owned(),
            }];
            assert_eq!(validate_env(&env), Err(LaunchParameterError::Environment));
        }
        assert!(
            validate_env(&[EnvVar {
                name: "MY_JAVA_OPTIONS".to_owned(),
                value: "keep this".to_owned(),
            }])
            .is_ok()
        );
    }

    #[test]
    fn version_must_use_the_verified_identity_instead_of_hard_coded_values() {
        let valid = [
            "--username",
            "${auth_player_name}",
            "--uuid",
            "${auth_uuid}",
            "--accessToken",
            "${auth_access_token}",
        ];
        assert!(validate_version_game(&valid).is_ok());
        assert!(
            validate_version_game(&[
                "--username",
                "${auth_player_name}",
                "--session",
                "${auth_session}"
            ])
            .is_ok()
        );

        let mut duplicate = valid.to_vec();
        duplicate.extend(["--accessToken", "0"]);
        assert_eq!(
            validate_version_game(&duplicate),
            Err(LaunchParameterError::Version)
        );
        let spoofed = ["--username", "Other", "--session", "${auth_session}"];
        assert_eq!(
            validate_version_game(&spoofed),
            Err(LaunchParameterError::Version)
        );
        assert_eq!(
            validate_version_game(&["--fullscreen"]),
            Err(LaunchParameterError::Version)
        );
    }

    #[test]
    fn standard_loader_jvm_module_flags_work_but_alternate_auth_is_rejected() {
        let loader_args = [
            "-DlibraryDirectory=${library_directory}",
            "-cp",
            "${classpath}",
            "-p",
            "${library_directory}/loader.jar",
            "--add-modules",
            "ALL-MODULE-PATH",
            "--add-opens=java.base/java.lang=ALL-UNNAMED",
        ];
        assert!(validate_version_jvm(loader_args.into_iter()).is_ok());
        let blocked = loader_args
            .into_iter()
            .chain(["-javaagent:authlib-injector.jar"]);
        assert_eq!(
            validate_version_jvm(blocked),
            Err(LaunchParameterError::Version)
        );
    }
}
