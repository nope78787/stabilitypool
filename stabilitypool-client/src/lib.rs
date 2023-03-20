use fedimint_client::module::gen::ClientModuleGen;
use fedimint_client::module::ClientModule;
use fedimint_client::sm::{DynState, OperationId, State, StateTransition};
use fedimint_core::core::{IntoDynInstance, ModuleInstanceId};
use fedimint_core::db::Database;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::ExtendsCommonModuleGen;
use fedimint_core::{apply, async_trait_maybe_send};
use stabilitypool::common::PoolModuleTypes;
use stabilitypool::config::PoolConfigClient;
use stabilitypool::PoolCommonGen;

#[derive(Debug, Clone)]
pub struct PoolClientGen;

impl ExtendsCommonModuleGen for PoolClientGen {
    type Common = PoolCommonGen;
}

#[apply(async_trait_maybe_send!)]
impl ClientModuleGen for PoolClientGen {
    type Module = PoolClientModule;
    type Config = PoolConfigClient;

    async fn init(&self, _cfg: Self::Config, _db: Database) -> anyhow::Result<Self::Module> {
        Ok(PoolClientModule {})
    }
}

#[derive(Debug)]
pub struct PoolClientModule {}

impl ClientModule for PoolClientModule {
    type Common = PoolModuleTypes;
    type ModuleStateMachineContext = ();
    type GlobalStateMachineContext = ();
    type States = PoolClientStates;

    fn context(&self) -> Self::ModuleStateMachineContext {
        unimplemented!()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub enum PoolClientStates {}

impl IntoDynInstance for PoolClientStates {
    type DynType = DynState<()>;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}

impl State<()> for PoolClientStates {
    type ModuleContext = ();

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        _global_context: &(),
    ) -> Vec<StateTransition<Self>> {
        unimplemented!()
    }

    fn operation_id(&self) -> OperationId {
        unimplemented!()
    }
}
