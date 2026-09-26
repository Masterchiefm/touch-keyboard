//! 新版本检测: 查询 GitHub 最新 Release → 版本比较 → 通知前端。
//!
//! 静默原则 (同 zcode-speed-panel 的 updater): 自动检查路径上任何失败
//! (断网、限流、解析失败)都只是"没有更新"—— 宁可漏报, 不打扰用户;
//! 只有用户手动点了「检查更新」才如实反馈失败原因。
//!
//! Linux 没有安全的自动安装路径 (deb 需 root、AppImage 替换二进制因安装
//! 方式而异), 因此本模块只做「检测」: 发现新版本后由前端展示卡片,
//! 「查看更新」按钮经 open_url 命令用系统浏览器打开 Release 页面手动下载。
//!
//! 本模块不依赖 tauri (纯函数 + std + HTTP), 事件与线程编排在 main.rs。

use std::time::Duration;

/// GitHub 仓库 (与 git remote、CI Release 一致)
pub const REPO: &str = "Masterchiefm/touch-keyboard";

/// 成功解析的最新 Release
#[derive(Clone, Debug)]
pub struct Release {
    /// Release tag, 如 "v0.1.0"
    pub tag: String,
    /// 去 v 前缀的版本号, 如 "0.1.0"
    pub version: String,
    /// Release 页面链接 (「查看更新」用系统浏览器打开)
    pub url: String,
    /// Release body (更新说明)
    pub notes: String,
}

/// "v0.3.1" / "0.3.1" → (0, 3, 1)。v 前缀可省; `-rc.1` 先行版本后缀与
/// `+build` 元数据忽略 (本项目不发预发布); 任一段非数字或超三段 → None
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut it = core.split('.');
    let maj = it.next()?.parse().ok()?;
    let min = it.next().unwrap_or("0").parse().ok()?;
    let pat = it.next().unwrap_or("0").parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((maj, min, pat))
}

/// candidate 是否比 current 新。任一侧解析失败 → false: 解析不了的 tag
/// 不能当成新版本诱导用户"更新"
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// 查询最新 Release (GitHub API 的 latest 端点天然排除 draft/prerelease)。
/// 任何失败 (断网、403 限流、解析失败) → None, 自动检查路径静默
pub fn fetch_latest(user_agent: &str) -> Option<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = http_agent()
        .get(&url)
        .set("User-Agent", user_agent) // API 对无 UA 的请求直接拒绝
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .call()
        .ok()?;
    let v: serde_json::Value = resp.into_json().ok()?;
    let tag = v.get("tag_name")?.as_str()?.trim().to_string();
    let html_url = v.get("html_url")?.as_str()?.to_string();
    let notes = v.get("body").and_then(|b| b.as_str()).unwrap_or("").trim().to_string();
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        tag,
        url: html_url,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("v0.3.1"), Some((0, 3, 1)));
        assert_eq!(parse_version("0.3.1"), Some((0, 3, 1)));
        assert_eq!(parse_version("V1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.3.1-rc.1"), Some((0, 3, 1))); // 先行版本后缀忽略
        assert_eq!(parse_version("v0.3.1+build.2"), Some((0, 3, 1)));
        assert_eq!(parse_version("v10.0.0"), Some((10, 0, 0)));
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn version_ordering() {
        assert!(is_newer("v0.3.0", "0.2.1"));
        assert!(is_newer("v0.2.2", "v0.2.1"));
        assert!(is_newer("v1.0.0", "v0.99.99"));
        assert!(!is_newer("v0.2.1", "0.2.1")); // 相等不算更新
        assert!(!is_newer("v0.2.0", "v0.2.1"));
        assert!(!is_newer("garbage", "0.2.1")); // 解析失败宁可漏报
        assert!(!is_newer("0.2.1", "garbage"));
    }
}
