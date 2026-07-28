use crate::Market;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenseTier {
    UnlicensedFreeTier,
    FirstParty,
    LicensedCommercial,
}

#[derive(Clone, Debug)]
pub struct AdapterAuthorization {
    pub license_tier: LicenseTier,
    pub commercial_use_allowed: bool,
    pub latency_p95_ms: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct AdapterDescriptor {
    pub id: &'static str,
    pub authorization: AdapterAuthorization,
    pub quality_rank: u8,
    pub markets: &'static [Market],
}

impl AdapterDescriptor {
    #[must_use]
    pub fn supports(&self, market: Market) -> bool {
        self.markets.contains(&market)
    }
}

#[must_use]
pub fn select_adapter_chain(
    candidates: &[AdapterDescriptor],
    market: Market,
    commercial_mode: bool,
) -> Vec<&AdapterDescriptor> {
    let mut eligible: Vec<_> = candidates
        .iter()
        .filter(|adapter| {
            adapter.supports(market)
                && (!commercial_mode || adapter.authorization.commercial_use_allowed)
        })
        .collect();
    eligible.sort_by_key(|adapter| {
        (
            adapter.quality_rank,
            adapter.authorization.latency_p95_ms.unwrap_or(u32::MAX),
        )
    });
    eligible
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: &[Market] = &[Market::Us];

    fn adapter(id: &'static str, rank: u8, commercial: bool, latency: u32) -> AdapterDescriptor {
        AdapterDescriptor {
            id,
            authorization: AdapterAuthorization {
                license_tier: if commercial {
                    LicenseTier::LicensedCommercial
                } else {
                    LicenseTier::UnlicensedFreeTier
                },
                commercial_use_allowed: commercial,
                latency_p95_ms: Some(latency),
            },
            quality_rank: rank,
            markets: US,
        }
    }

    #[test]
    fn commercial_mode_never_falls_back_to_unlicensed_source() {
        let candidates = [adapter("free", 1, false, 10), adapter("paid", 2, true, 50)];
        let chain = select_adapter_chain(&candidates, Market::Us, true);
        assert_eq!(
            chain.iter().map(|item| item.id).collect::<Vec<_>>(),
            ["paid"]
        );
    }

    #[test]
    fn rank_then_latency_is_deterministic() {
        let candidates = [adapter("slow", 1, true, 100), adapter("fast", 1, true, 30)];
        let chain = select_adapter_chain(&candidates, Market::Us, false);
        assert_eq!(chain[0].id, "fast");
    }
}
