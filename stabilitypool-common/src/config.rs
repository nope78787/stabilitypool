use crate::price::{BitMexOracle, MockOracle, OracleClient};
use crate::stability_core::CollateralRatio;
use crate::FileOracle;
use crate::PoolCommonGen;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::{core::ModuleKind, plugin_types_trait_impl_config};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use time::OffsetDateTime;

/// The default epoch length is 24hrs (represented in seconds).
// pub const DEFAULT_EPOCH_LENGTH: u64 = 24 * 60 * 60;
pub const DEFAULT_EPOCH_LENGTH: u64 = 40; // TODO: This is just for testing

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Configuration that will be encrypted.
    pub private: PoolConfigPrivate,
    /// Configuration that needs to be the same for every federation member.
    pub consensus: PoolConfigConsensus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfigPrivate {}

#[derive(Clone, Debug, Serialize, Deserialize, Encodable, Decodable)]
pub struct PoolConfigConsensus {
    pub epoch: EpochConfig,
    pub oracle: OracleConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encodable, Decodable)]
pub enum OracleConfig {
    BitMex,
    Mock(String),
    File(String),
}

impl Default for OracleConfig {
    fn default() -> Self {
        OracleConfig::File("./misc/offline_oracle".to_string())
    }
}

impl OracleConfig {
    pub fn oracle_client(&self) -> Box<dyn OracleClient> {
        match self {
            OracleConfig::BitMex => Box::new(BitMexOracle {}),
            OracleConfig::Mock(url) => Box::new(MockOracle {
                url: reqwest::Url::parse(url).expect("invalid Url"),
            }),
            OracleConfig::File(path) => {
                let path = PathBuf::from_str(&path).expect("must be valid path");
                Box::new(FileOracle { path })
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Encodable, Decodable)]
pub struct EpochConfig {
    pub start_epoch_at: u64,
    pub epoch_length: u64,
    /// Number of peers that have to agree on price before it's used
    pub price_threshold: u32,
    /// The maximum a provider can charge per epoch in parts per million of
    /// locked principal
    pub max_feerate_ppm: u64,
    /// The ratio of seeker position to provider collateral
    pub collateral_ratio: CollateralRatio,
}

impl EpochConfig {
    pub fn epoch_id_for_time(&self, time: OffsetDateTime) -> u64 {
        if time < self.start_epoch_at() {
            0
        } else {
            (time - self.start_epoch_at()).whole_seconds() as u64 / self.epoch_length + 1
        }
    }

    pub fn start_epoch_at(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(self.start_epoch_at as _)
            .expect("must be valid unix timestamp")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encodable, Decodable)]
pub struct PoolClientConfig {
    pub oracle: OracleConfig,
    pub collateral_ratio: CollateralRatio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfigGenParams {
    pub important_param: u64,
    #[serde(default)]
    pub start_epoch_at: Option<time::PrimitiveDateTime>,
    /// this is in seconds
    pub epoch_length: u64,
    pub oracle_config: OracleConfig,
    /// The ratio of seeker position to provider collateral
    #[serde(default)]
    pub collateral_ratio: CollateralRatio,
}

impl Default for PoolConfigGenParams {
    fn default() -> Self {
        Self {
            important_param: 3,
            start_epoch_at: None,
            epoch_length: DEFAULT_EPOCH_LENGTH,
            oracle_config: OracleConfig::default(),
            collateral_ratio: Default::default(),
        }
    }
}
// impl TypedServerModuleConfig for PoolConfig {
//     type Local = ();
//     type Private = PoolConfigPrivate;
//     type Consensus = PoolConfigConsensus;

//     fn from_parts(_: Self::Local, private: Self::Private, consensus: Self::Consensus) -> Self {
//         Self { private, consensus }
//     }

//     fn to_parts(
//         self,
//     ) -> (
//         ModuleKind,
//         Self::Local,
//         Self::Private,
//         Self::Consensus,
//     ) {
//         (KIND, (), self.private, self.consensus)
//     }

// }

// impl TypedServerModuleConsensusConfig for PoolConfigConsensus {
//     fn kind(&self) -> ModuleKind {
//         KIND
//     }

//     fn version(&self) -> ModuleConsensusVersion {
//         ModuleConsensusVersion(0)
//     }

//     fn to_client_config(&self) -> fedimint_core::config::ClientModuleConfig {
//         fedimint_core::config::ClientModuleConfig::from_typed(
//             KIND,
//             &PoolConfigClient {
//                 oracle: self.oracle.clone(),
//                 collateral_ratio: self.epoch.collateral_ratio,
//             },
//         )
//         .expect("serialization cannot fail")
//     }
// }

// impl TypedClientModuleConfig for PoolConfigClient {
//     fn kind(&self) -> fedimint_core::core::ModuleKind {
//         KIND
//     }
// }

plugin_types_trait_impl_config!(
    PoolCommonGen,
    PoolConfigGenParams,
    PoolConfig,
    PoolConfigPrivate,
    PoolConfigConsensus,
    PoolClientConfig
);
