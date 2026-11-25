use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use semver::Version;
use serde_json::Value;

use crate::process::{Result, run};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Package {
    Pi,
    Web,
}

impl Package {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pi => "Pi CLI",
            Self::Web => "Pi Web",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Pi => "@earendil-works/pi-coding-agent",
            Self::Web => "@agegr/pi-web",
        }
    }

    fn bin(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Web => "pi-web",
        }
    }
}

pub struct Runtime {
    node: PathBuf,
    npm: PathBuf,
    root: PathBuf,
    path: OsString,
    home: PathBuf,
}

pub struct Installed {
    pub entry: PathBuf,
    pub version: Version,
    pub internal_pi: Option<String>,
}

impl Runtime {
    pub fn find() -> Result<Self> {
        let home = dirs::home_dir().ok_or("找不到用户目录。")?;
        let paths = search_paths(&home);
        let path = env::join_paths(&paths).map_err(|e| format!("PATH 无效：{e}"))?;
        let node_name = if cfg!(windows) { "node.exe" } else { "node" };
        let node = paths
            .iter()
            .map(|p| p.join(node_name))
            .find(|p| p.is_file())
            .ok_or("找不到 Node.js。请安装 Node.js 22.19 或更高版本。")?;
        let mut runtime = Self {
            node,
            npm: PathBuf::new(),
            root: PathBuf::new(),
            path,
            home,
        };
        let mut command = runtime.node_command();
        command.arg("--version");
        let version = run(command, "读取 Node.js 版本", Duration::from_secs(5))?;
        let version = Version::parse(version.trim().trim_start_matches('v'))
            .map_err(|e| format!("不能解析 Node.js 版本：{e}"))?;
        if version < Version::new(22, 19, 0) {
            return Err(format!(
                "Node.js {version} 太旧；请安装 Node.js 22.19 或更高版本。"
            ));
        }
        runtime.npm =
            npm_entry(&runtime.node, &paths).ok_or("找不到 npm。请安装随 Node.js 提供的 npm。")?;
        let root = run(
            runtime.npm_command(["root", "--global"]),
            "查找全局 npm 安装目录",
            Duration::from_secs(12),
        )?;
        runtime.root = PathBuf::from(root.trim());
        if !runtime.root.is_absolute() {
            return Err("npm 返回的全局安装目录无效。".into());
        }
        Ok(runtime)
    }

    fn node_command(&self) -> Command {
        let mut command = Command::new(&self.node);
        command.env("PATH", &self.path).current_dir(&self.home);
        command
    }

    fn npm_command(&self, args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> Command {
        let mut command = self.node_command();
        command.arg(&self.npm).args(args);
        command
    }

    pub fn command(&self, installed: &Installed) -> Command {
        let mut command = self.node_command();
        command.arg(&installed.entry);
        command
    }

    pub fn installed(&self, package: Package) -> Result<Installed> {
        let dir = self.root.join(package.name());
        if !dir.join("package.json").is_file() {
            return Err(format!(
                "没有找到全局 npm 安装的 {}。\n可在终端安装：npm install -g {}",
                package.label(),
                package.name()
            ));
        }
        // Don't run or overwrite a global link to a checkout or a different installation.
        let expected = canonical(&self.root)?.join(package.name());
        if canonical(&dir)? != expected {
            return Err(format!(
                "{} 是链接安装，请在原安装环境中管理。",
                package.label()
            ));
        }
        read_package(&expected, package)
    }

    pub fn latest(&self, package: Package) -> Result<Version> {
        let name = format!("{}@latest", package.name());
        let output = run(
            self.npm_command([
                "view",
                &name,
                "version",
                "--json",
                "--global",
                "--prefer-online",
                "--fetch-retries=0",
                "--fetch-timeout=10000",
            ]),
            "查询 npm 版本",
            Duration::from_secs(18),
        )?;
        let version: String = serde_json::from_str(&output).map_err(|_| "npm 返回的版本无效。")?;
        Version::parse(&version).map_err(|e| format!("npm 返回的版本无效：{e}"))
    }

    pub fn ensure_web_stopped(&self, installed: &Installed) -> Result<()> {
        let mut command = self.command(installed);
        command.arg("status");
        let output = run(command, "检查其他 Pi Web 进程", Duration::from_secs(10))?;
        if output.trim() != "No pi-web server is running." {
            return Err("另有 Pi Web 服务运行，或无法确认其状态。请在终端检查 `pi-web status` 并停止外部实例后再更新。".into());
        }
        Ok(())
    }

    pub fn install(&self, package: Package, version: &Version) -> Result<Installed> {
        let target = format!("{}@{version}", package.name());
        run(
            self.npm_command(["install", "--global", "--no-progress", &target]),
            "npm 更新",
            Duration::from_secs(300),
        )?;
        let installed = self.installed(package)?;
        if &installed.version != version {
            return Err(format!(
                "更新后检测到 {}，不是所选版本 {version}。",
                installed.version
            ));
        }
        Ok(installed)
    }
}

fn canonical(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))
}

fn read_package(dir: &Path, package: Package) -> Result<Installed> {
    let manifest = fs::read(dir.join("package.json"))
        .map_err(|e| format!("读取 {} 元数据失败：{e}", package.label()))?;
    let data: Value = serde_json::from_slice(&manifest)
        .map_err(|e| format!("{} 元数据无效：{e}", package.label()))?;
    if data["name"].as_str() != Some(package.name()) {
        return Err(format!("{} 的实际包名不符，拒绝运行。", package.label()));
    }
    let version = data["version"]
        .as_str()
        .and_then(|v| Version::parse(v).ok())
        .ok_or("安装包版本无效。")?;
    let bin = data["bin"][package.bin()]
        .as_str()
        .ok_or("安装包缺少命令入口。")?;
    let entry = canonical(&dir.join(bin))?;
    if !entry.starts_with(dir) || !entry.is_file() {
        return Err("命令入口不在安装包内。".into());
    }
    let internal_pi = data["dependencies"][Package::Pi.name()]
        .as_str()
        .map(str::to_owned);
    Ok(Installed {
        entry,
        version,
        internal_pi,
    })
}

fn npm_entry(node: &Path, paths: &[PathBuf]) -> Option<PathBuf> {
    let mut bins = vec![node.parent()?.to_path_buf()];
    if let Ok(resolved) = fs::canonicalize(node)
        && let Some(parent) = resolved.parent()
    {
        bins.push(parent.to_path_buf());
    }
    bins.extend_from_slice(paths);
    bins.into_iter().find_map(|bin| {
        [
            bin.join("node_modules/npm/bin/npm-cli.js"),
            bin.join("../lib/node_modules/npm/bin/npm-cli.js"),
            bin.join("npm"),
        ]
        .into_iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .find(|p| p.file_name().is_some_and(|file| file == "npm-cli.js") && p.is_file())
    })
}

fn search_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = env::var_os("PATH")
        .iter()
        .flat_map(env::split_paths)
        .filter(|p| p.is_absolute())
        .collect();
    if cfg!(windows) {
        if let Some(program_files) = env::var_os("ProgramFiles") {
            paths.push(PathBuf::from(program_files).join("nodejs"));
        }
        if let Some(appdata) = env::var_os("APPDATA") {
            paths.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        paths.extend([
            home.join(".local/bin"),
            home.join(".volta/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
            "/bin".into(),
        ]);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_package_identities_and_version_order() {
        assert_eq!(Package::Web.name(), "@agegr/pi-web");
        assert_eq!(Package::Pi.name(), "@earendil-works/pi-coding-agent");
        assert!(Version::parse("0.18.0").unwrap() > Version::parse("0.9.0").unwrap());
    }

    #[test]
    fn rejects_an_unrelated_package_before_using_its_entry() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        fs::write(
            dir.join("package.json"),
            r#"{"name":"pi-web","version":"9.9.9","bin":{"pi-web":"anything"}}"#,
        )
        .unwrap();
        assert!(
            read_package(dir, Package::Web)
                .err()
                .unwrap()
                .contains("包名不符")
        );
    }
}
