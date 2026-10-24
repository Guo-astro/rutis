//! aimux-llm 的 rutis 插件形态:apply → 注册 `dyn LlmService`。

use std::sync::Arc;

use rutis::host::Key;
use rutis::{Ctx, Plugin};

use crate::provider::AimuxLlm;
use crate::service::LlmService;

/// 服务键(`ctx.get::<dyn LlmService>()`,或宿主 `get_at(&llm_service_key())` 取回)。
pub fn llm_service_key() -> Key<dyn LlmService> {
    Key::of()
}

/// 装配单元:持服务实例,apply 注册进当前 ctx。
pub struct AimuxLlmPlugin {
    service: Arc<dyn LlmService>,
}

impl AimuxLlmPlugin {
    /// env 兜底构造(见 [`AimuxLlm::from_env`])。
    pub fn from_env() -> Self {
        Self {
            service: Arc::new(AimuxLlm::from_env()),
        }
    }

    /// 服务注入版(测试/组合根定制)。
    pub fn with_service(service: Arc<dyn LlmService>) -> Self {
        Self { service }
    }
}

impl Plugin for AimuxLlmPlugin {
    type Deps = ();

    fn name(&self) -> &str {
        "aimux-llm"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide_as::<dyn LlmService>(Arc::clone(&self.service))?;
        Ok(())
    }
}
