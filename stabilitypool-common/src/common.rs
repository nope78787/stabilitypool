use fedimint_core::module::ModuleCommon;
use fedimint_core::plugin_types_trait_impl_common;

use crate::{ModuleInstanceId, PoolConsensusItem, PoolInput, PoolOutput, PoolOutputOutcome};

// #[derive(Debug, Default, Clone)]
// pub struct PoolDecoder;

pub struct StabilityPoolModuleTypes;

impl ModuleCommon for StabilityPoolModuleTypes {
    type Input = PoolInput;
    type Output = PoolOutput;
    type OutputOutcome = PoolOutputOutcome;
    type ConsensusItem = PoolConsensusItem;
}

plugin_types_trait_impl_common!(PoolInput, PoolOutput, PoolOutputOutcome, PoolConsensusItem);
