use fedimint_core::encoding::{Decodable, Encodable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Encodable, Decodable)]
pub enum LockedBalance {
    #[serde(rename = "seeker")]
    Seeker(fedimint_core::Amount),
    #[serde(rename = "provider")]
    Provider(fedimint_core::Amount),
    #[serde(rename = "none")]
    None,
}

impl LockedBalance {
    pub fn amount(self) -> fedimint_core::Amount {
        match self {
            LockedBalance::Seeker(a) => a,
            LockedBalance::Provider(a) => a,
            LockedBalance::None => fedimint_core::Amount::ZERO,
        }
    }
}

/// TODO: Add `last_seq`, `last_epoch`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encodable, Decodable)]
pub struct AccountBalance {
    pub unlocked: fedimint_core::Amount,
    pub locked: LockedBalance,
}

impl Default for AccountBalance {
    fn default() -> Self {
        Self {
            unlocked: fedimint_core::Amount::ZERO,
            locked: LockedBalance::None,
        }
    }
}

impl AccountBalance {
    /// Obtain total balance with overflow checks. Returns [`None`] on overflow.
    pub fn total_balance(&self) -> Option<fedimint_core::Amount> {
        [self.unlocked.msats, self.locked.amount().msats]
            .iter()
            .try_fold(0_u64, |acc, &v| acc.checked_add(v))
            .map(fedimint_core::msats)
    }

    /// Determines whether an amount can be added without overflow.
    pub fn can_add_amount(&self, amount: fedimint_core::Amount) -> bool {
        [
            self.unlocked.msats,
            self.locked.amount().msats,
            amount.msats,
        ]
        .iter()
        .try_fold(0_u64, |acc, &v| acc.checked_add(v))
        .map(fedimint_core::msats)
        .is_some()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Encodable, Decodable, PartialEq, Eq, Hash)]
pub struct AccountDeposit {
    pub account: bitcoin::XOnlyPublicKey,
    pub amount: fedimint_core::Amount,
}

impl core::fmt::Display for AccountDeposit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "+{}@{}", self.amount, self.account)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Encodable, Decodable, PartialEq, Eq, Hash)]
pub struct AccountWithdrawal {
    pub account: bitcoin::XOnlyPublicKey,
    pub amount: fedimint_core::Amount,
}

impl core::fmt::Display for AccountWithdrawal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "-{}@{}", self.amount, self.account)
    }
}
