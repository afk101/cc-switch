use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::constants::WISGPT_FAST_MODE_CONFIG_PATH;
use crate::error::AppError;

/// 与 7072 共享的文件格式。缺失、损坏或值类型错误时，两侧均视为关闭。
#[derive(Deserialize, Serialize)]
struct FastModeConfig {
    wisgpt_fast_mode: bool,
}

/// 从服务账号家目录读取开关，每次打开托盘都能看到外部修改。
pub fn read_enabled(home: &Path) -> bool {
    std::fs::read(home.join(WISGPT_FAST_MODE_CONFIG_PATH))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<FastModeConfig>(&bytes).ok())
        .is_some_and(|config| config.wisgpt_fast_mode)
}

/// 原子替换开关文件，避免代理恰好在写入时读到半个 JSON。
pub fn write_enabled(home: &Path, enabled: bool) -> Result<(), AppError> {
    crate::config::write_json_file_private(
        &home.join(WISGPT_FAST_MODE_CONFIG_PATH),
        &FastModeConfig {
            wisgpt_fast_mode: enabled,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_both_states_in_the_file_read_by_7072() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".config/claude-openai-proxy/7072.json");
        assert!(!read_enabled(home.path()));
        for enabled in [true, false] {
            write_enabled(home.path(), enabled).unwrap();
            let disk: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(disk, serde_json::json!({"wisgpt_fast_mode": enabled}));
            assert_eq!(read_enabled(home.path()), enabled);
        }
        std::fs::write(path, br#"{"wisgpt_fast_mode":"true"}"#).unwrap();
        assert!(!read_enabled(home.path()));
    }
}
