mod cli;

use std::ffi;

use async_trait::async_trait;
use common::action::SignedAction;
use common::config::PoolClientConfig;
use common::Action;
use common::ActionProposed;
use common::ActionStaged;
use common::EpochOutcome;
use common::PoolCommonGen;
use common::PoolModuleTypes;
use common::ProviderBid;
use common::SeekerAction;

use fedimint_client::derivable_secret::DerivableSecret;
use fedimint_client::module::gen::ClientModuleGen;
use fedimint_client::module::ClientModule;
use fedimint_client::sm::{DynState, ModuleNotifier, OperationId, State, StateTransition};

use fedimint_client::Client;
use fedimint_client::DynGlobalClientContext;
use fedimint_core::api::FederationApiExt;
use fedimint_core::api::FederationError;
use fedimint_core::core::{IntoDynInstance, ModuleInstanceId};
use fedimint_core::db::Database;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::ApiRequestErased;
use fedimint_core::module::ExtendsCommonModuleGen;
use fedimint_core::BitcoinHash;
use fedimint_core::{apply, async_trait_maybe_send};
use secp256k1_zkp::Secp256k1;

use secp256k1::KeyPair;

use stabilitypool_common as common;
use stabilitypool_server::api::BalanceResponse;

#[derive(Debug, Clone)]
pub struct PoolClientGen;

impl ExtendsCommonModuleGen for PoolClientGen {
    type Common = PoolCommonGen;
}

#[apply(async_trait_maybe_send!)]
impl ClientModuleGen for PoolClientGen {
    type Module = PoolClientModule;
    type Config = PoolClientConfig;

    async fn init(
        &self,
        cfg: Self::Config,
        _db: Database,
        module_root_secret: DerivableSecret,
        _notifier: ModuleNotifier<DynGlobalClientContext, <Self::Module as ClientModule>::States>,
    ) -> anyhow::Result<Self::Module> {
        Ok(PoolClientModule {
            cfg,
            key: module_root_secret.to_secp_key(&Secp256k1::new()),
        })
    }
}

#[derive(Debug)]
pub struct PoolClientModule {
    cfg: PoolClientConfig,
    key: secp256k1_zkp::KeyPair,
}

#[async_trait]
impl ClientModule for PoolClientModule {
    type Common = PoolModuleTypes;
    type ModuleStateMachineContext = ();
    type States = PoolClientStates;

    fn context(&self) -> Self::ModuleStateMachineContext {
        unimplemented!()
    }

    fn input_amount(
        &self,
        _input: &<Self::Common as fedimint_core::module::ModuleCommon>::Input,
    ) -> fedimint_core::module::TransactionItemAmount {
        todo!()
    }

    fn output_amount(
        &self,
        _output: &<Self::Common as fedimint_core::module::ModuleCommon>::Output,
    ) -> fedimint_core::module::TransactionItemAmount {
        todo!()
    }

    async fn handle_cli_command(
        &self,
        client: &Client,
        args: &[ffi::OsString],
    ) -> anyhow::Result<serde_json::Value> {
        let rng = rand::rngs::OsRng::default();
        let output = cli::handle_cli_args(client, rng, args).await?;
        Ok(serde_json::to_value(&output).expect("infallible"))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub enum PoolClientStates {}

impl IntoDynInstance for PoolClientStates {
    type DynType = DynState<DynGlobalClientContext>;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}

impl State for PoolClientStates {
    type ModuleContext = ();
    type GlobalContext = DynGlobalClientContext;

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        _global_context: &DynGlobalClientContext,
    ) -> Vec<StateTransition<Self>> {
        unimplemented!()
    }

    fn operation_id(&self) -> OperationId {
        unimplemented!()
    }
}

#[apply(async_trait_maybe_send!)]
pub trait PoolClientExt {
    fn account_key(&self) -> KeyPair;

    async fn balance(&self) -> Result<BalanceResponse, FederationError>;

    async fn epoch_outcome(&self, epoch_id: u64) -> Result<EpochOutcome, FederationError>;

    async fn staging_epoch(&self) -> Result<u64, FederationError>;

    async fn create_signed_acton<T: Encodable + Send>(
        &self,
        unsigned_action: T,
    ) -> Result<SignedAction<T>, FederationError>;

    async fn propose_seeker_action(&self, action: SeekerAction) -> Result<(), FederationError>;

    async fn propose_provider_action(&self, action: ProviderBid) -> Result<(), FederationError>;

    async fn staged_action(&self) -> Result<ActionStaged, FederationError>;

    async fn state(&self) -> Result<stabilitypool_server::api::State, FederationError>;
}

#[apply(async_trait_maybe_send!)]
impl PoolClientExt for Client {
    fn account_key(&self) -> KeyPair {
        let (pool_client, _pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_client.key
    }

    async fn balance(&self) -> Result<BalanceResponse, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/account", common::KIND),
                ApiRequestErased::new(&self.account_key().x_only_public_key().0),
            )
            .await
    }

    async fn epoch_outcome(&self, epoch_id: u64) -> Result<EpochOutcome, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/epoch", common::KIND),
                ApiRequestErased::new(&epoch_id),
            )
            .await
    }

    async fn staging_epoch(&self) -> Result<u64, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/epoch_next", common::KIND),
                ApiRequestErased::default(),
            )
            .await
    }

    async fn create_signed_acton<T: Encodable + Send>(
        &self,
        unsigned_action: T,
    ) -> Result<SignedAction<T>, FederationError> {
        let kp = self.account_key();
        let sequence = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
        let action = Action {
            epoch_id: self.staging_epoch().await?,
            sequence,
            account_id: kp.x_only_public_key().0,
            body: unsigned_action,
        };

        let digest =
            bitcoin::hashes::sha256::Hash::hash(&action.consensus_encode_to_vec().unwrap());
        let signature = Secp256k1::signing_only().sign_schnorr(&digest.into(), &kp);
        let signed_action = SignedAction { signature, action };
        Ok(signed_action)
    }

    async fn propose_seeker_action(&self, action: SeekerAction) -> Result<(), FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        let signed_action: ActionProposed = self
            .create_signed_acton(action)
            .await
            .expect("TODO: signing should not fail")
            .into();
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/action_propose", common::KIND),
                ApiRequestErased::new(&signed_action),
            )
            .await
    }

    async fn propose_provider_action(&self, action: ProviderBid) -> Result<(), FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        let signed_action: ActionProposed = self
            .create_signed_acton(action)
            .await
            .expect("TODO: signing should not fail")
            .into();
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/action_propose", common::KIND),
                ApiRequestErased::new(&signed_action),
            )
            .await
    }

    async fn staged_action(&self) -> Result<ActionStaged, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/action", common::KIND),
                ApiRequestErased::new(&self.account_key().x_only_public_key().0),
            )
            .await
    }

    async fn state(&self) -> Result<stabilitypool_server::api::State, FederationError> {
        let (_pool_client, pool_instance) =
            self.get_first_module::<PoolClientModule>(&common::KIND);
        pool_instance
            .api
            .request_current_consensus(
                format!("/module/{}/state", common::KIND),
                ApiRequestErased::default(),
            )
            .await
    }
}
