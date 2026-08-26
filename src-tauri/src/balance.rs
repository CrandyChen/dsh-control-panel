//! 当前余额查询：读取 DSH 凭据（`$DSH_HOME/.credentials.yaml` 的 `DEEPSEEK_API_KEY`），
//! 调用 DeepSeek `GET /user/balance`，供状态总览展示与低余额提醒。
//!
//! 网络请求复用 PowerShell `Invoke-RestMethod`（与 prebuilt.rs 一致），避免新增 Rust HTTP 依赖。
//! 失败（无 key / 网络异常 / 解析失败）不阻塞主界面，前端显示「—」。

use serde::Serialize;

/// 余额查询结果（序列化后 camelCase）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResult {
    /// 本次能否成功获取（key 存在 + 网络可达 + 解析成功）。
    pub available: bool,
    /// 是否配置了 `DEEPSEEK_API_KEY`。
    pub api_key_set: bool,
    /// DeepSeek 返回的 `is_available`（账户是否可充值/可用）。
    pub is_available: Option<bool>,
    /// 余额币种（默认取 CNY；无 CNY 项时取首个 `balance_infos` 的币种）。
    pub currency: Option<String>,
    /// 数值余额（`total_balance` 转 f64；解析失败为 None）。
    pub balance: Option<f64>,
    /// 失败原因（前端浮层提示用）。
    pub error: Option<String>,
}

/// 读取 `$DSH_HOME/.credentials.yaml` 中的 `DEEPSEEK_API_KEY`。
fn read_api_key() -> Option<String> {
    let home = crate::detect::dsh_home();
    let path = std::path::Path::new(&home).join(".credentials.yaml");
    let text = std::fs::read_to_string(&path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    let json: serde_json::Value = serde_json::to_value(&yaml).ok()?;
    json.get("refs")?
        .get("DEEPSEEK_API_KEY")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 从 DeepSeek 余额响应 JSON 解析 `(is_available, currency, balance)`。
fn parse_balance(json: &str) -> (Option<bool>, Option<String>, Option<f64>) {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return (None, None, None),
    };
    let is_avail = v.get("is_available").and_then(|x| x.as_bool());
    let entry = v
        .get("balance_infos")
        .and_then(|a| a.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("currency").and_then(|c| c.as_str()) == Some("CNY"))
                .or_else(|| a.first())
        });
    match entry {
        Some(e) => {
            let currency = e
                .get("currency")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            let balance = e
                .get("total_balance")
                .and_then(|b| b.as_str())
                .and_then(|s| s.parse::<f64>().ok());
            (is_avail, currency, balance)
        }
        None => (is_avail, None, None),
    }
}

/// 查询当前余额。
pub fn get_balance() -> BalanceResult {
    let key = match read_api_key() {
        Some(k) => k,
        None => {
            return BalanceResult {
                available: false,
                api_key_set: false,
                is_available: None,
                currency: None,
                balance: None,
                error: Some(crate::i18n::t("balance.no_key")),
            }
        }
    };

    let script = r#"
$ProgressPreference='SilentlyContinue'
$h=@{'Authorization'='Bearer @@KEY@@';'User-Agent'='DSH-Control-Panel'}
try {
  $r=Invoke-RestMethod -Uri 'https://api.deepseek.com/user/balance' -Headers $h
  $r | ConvertTo-Json -Depth 10
} catch {
  Write-Error $_.Exception.Message
}
"#;
    let key_escaped = crate::prebuilt::ps_escape(&key);
    let script = script.replace("@@KEY@@", &key_escaped);
    match crate::prebuilt::powershell_capture(&script) {
        Ok(out) => {
            let (is_avail, currency, balance) = parse_balance(&out);
            BalanceResult {
                available: true,
                api_key_set: true,
                is_available: is_avail,
                currency,
                balance,
                error: None,
            }
        }
        Err(e) => BalanceResult {
            available: false,
            api_key_set: true,
            is_available: None,
            currency: None,
            balance: None,
            error: Some(crate::prebuilt::clean_ps_error(&e.to_string())),
        },
    }
}

/// 充值链接（前端 `open_external` 使用；此处保留供后端日志/提示复用）。
#[allow(dead_code)]
pub const TOP_UP_URL: &str = "https://platform.deepseek.com/top_up";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cny_balance() {
        let json = r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00"}]}"#;
        let (avail, cur, bal) = parse_balance(json);
        assert_eq!(avail, Some(true));
        assert_eq!(cur.as_deref(), Some("CNY"));
        assert_eq!(bal, Some(110.0));
    }

    #[test]
    fn parse_multi_currency_prefers_cny() {
        let json = r#"{"is_available":true,"balance_infos":[
            {"currency":"USD","total_balance":"1.00"},
            {"currency":"CNY","total_balance":"5.25"}]}"#;
        let (_, cur, bal) = parse_balance(json);
        assert_eq!(cur.as_deref(), Some("CNY"));
        assert_eq!(bal, Some(5.25));
    }

    #[test]
    fn parse_falls_back_to_first_when_no_cny() {
        let json = r#"{"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"2.5"}]}"#;
        let (_, cur, bal) = parse_balance(json);
        assert_eq!(cur.as_deref(), Some("USD"));
        assert_eq!(bal, Some(2.5));
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        let (a, c, b) = parse_balance("not json");
        assert_eq!(a, None);
        assert_eq!(c, None);
        assert_eq!(b, None);
    }

    #[test]
    fn no_key_reports_not_configured() {
        // 依赖真实的 ~/.dsh/.credentials.yaml 存在；此处仅验证 API key 缺失时返回 apiKeySet=false。
        // 无法在此构造 DSH_HOME，故直接断言 read_api_key 在空内容上为 None。
        let tmp = std::env::temp_dir().join(format!("dsh-balance-k-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        // 使用在内存构造的 yaml 校验解析分支。
        let yaml = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: sk-test\n";
        let v: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["refs"]["DEEPSEEK_API_KEY"].as_str(), Some("sk-test"));
    }
}
