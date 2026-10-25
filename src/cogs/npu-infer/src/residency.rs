//! Which models are loaded, what each costs, and what to evict to load
//! another. Pure bookkeeping: the scheduler asks for a plan, applies it
//! through the backend, then records the result here.
//!
//! A model is admitted when the sum of resident costs plus its own stays
//! within `budget_mib`, the resident count stays within `max_resident`, and
//! the system still has `reserve_mib` of `MemAvailable` left after it loads
//! (the reserve covers the image service, the daemon and builds). Room is
//! made by evicting the least recently used models that are not running.
//! Costs are the measured ones once a model has loaded (they are kept across
//! evictions), the catalog estimate before that.

use std::collections::HashMap;

use crate::catalog::Entry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    pub alias: String,
    pub cost_mib: u64,
    /// Logical clock of the last use; larger is more recent.
    pub last_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Already loaded.
    Hit,
    /// Evict these (oldest use first), then load.
    Load { evict: Vec<String>, cost_mib: u64 },
    /// The model alone is over the budget (or the reserve), whatever is evicted.
    TooBig { need_mib: u64, limit_mib: u64 },
}

pub struct Residency {
    pub budget_mib: u64,
    pub reserve_mib: u64,
    pub max_resident: usize,
    resident: Vec<Resident>,
    measured: HashMap<String, u64>,
    clock: u64,
}

impl Residency {
    pub fn new(budget_mib: u64, reserve_mib: u64, max_resident: usize) -> Self {
        Self {
            budget_mib,
            reserve_mib,
            max_resident: max_resident.max(1),
            resident: Vec::new(),
            measured: HashMap::new(),
            clock: 0,
        }
    }

    /// Measured costs from an earlier run.
    pub fn seed_costs(&mut self, costs: impl IntoIterator<Item = (String, u64)>) {
        self.measured.extend(costs);
    }

    pub fn measured(&self) -> &HashMap<String, u64> {
        &self.measured
    }

    pub fn cost_of(&self, e: &Entry) -> u64 {
        self.measured.get(&e.alias).copied().unwrap_or(e.est_mib)
    }

    pub fn used_mib(&self) -> u64 {
        self.resident.iter().map(|r| r.cost_mib).sum()
    }

    pub fn resident(&self) -> &[Resident] {
        &self.resident
    }

    pub fn is_resident(&self, alias: &str) -> bool {
        self.resident.iter().any(|r| r.alias == alias)
    }

    /// Mark `alias` used now.
    pub fn touch(&mut self, alias: &str) {
        self.clock += 1;
        let c = self.clock;
        if let Some(r) = self.resident.iter_mut().find(|r| r.alias == alias) {
            r.last_used = c;
        }
    }

    /// Plan for `e`; `mem_available_mib` is the system's current `MemAvailable`
    /// (`None` when unknown: only the budget is enforced).
    pub fn plan(&self, e: &Entry, mem_available_mib: Option<u64>) -> Plan {
        if self.is_resident(&e.alias) {
            return Plan::Hit;
        }
        let need = self.cost_of(e);
        let limit = match mem_available_mib {
            // Everything resident could be evicted, so the ceiling is the
            // smaller of the budget and what the system would have free.
            Some(avail) => self
                .budget_mib
                .min((avail + self.used_mib()).saturating_sub(self.reserve_mib)),
            None => self.budget_mib,
        };
        if need > limit {
            return Plan::TooBig {
                need_mib: need,
                limit_mib: limit,
            };
        }
        let mut by_age: Vec<&Resident> = self.resident.iter().collect();
        by_age.sort_by_key(|r| r.last_used);
        let mut evict = Vec::new();
        let mut used = self.used_mib();
        let mut count = self.resident.len();
        for r in by_age {
            let fits = used + need <= limit && count < self.max_resident;
            if fits {
                break;
            }
            used -= r.cost_mib;
            count -= 1;
            evict.push(r.alias.clone());
        }
        Plan::Load {
            evict,
            cost_mib: need,
        }
    }

    /// After `keep` loaded and its measured cost went in, the models to evict
    /// (oldest first) because the cost came out above the estimate and the
    /// book is over budget. `keep` is never a victim.
    pub fn over_budget(&self, keep: &str) -> Vec<String> {
        let mut by_age: Vec<&Resident> = self.resident.iter().filter(|r| r.alias != keep).collect();
        by_age.sort_by_key(|r| r.last_used);
        let mut used = self.used_mib();
        let mut out = Vec::new();
        for r in by_age {
            if used <= self.budget_mib {
                break;
            }
            used -= r.cost_mib;
            out.push(r.alias.clone());
        }
        out
    }

    pub fn remove(&mut self, alias: &str) {
        self.resident.retain(|r| r.alias != alias);
    }

    /// `alias` loaded and cost `cost_mib` (measured).
    pub fn add(&mut self, alias: &str, cost_mib: u64) {
        self.clock += 1;
        self.remove(alias);
        self.measured.insert(alias.to_string(), cost_mib);
        self.resident.push(Resident {
            alias: alias.to_string(),
            cost_mib,
            last_used: self.clock,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Kind;

    fn m(alias: &str, est: u64) -> Entry {
        Entry {
            alias: alias.into(),
            geniex_id: format!("x/{alias}"),
            kind: Kind::Llm,
            est_mib: est,
            nctx: 4096,
        }
    }

    #[test]
    fn a_resident_model_is_a_hit_and_a_fit_evicts_nothing() {
        let mut r = Residency::new(8000, 1000, 3);
        let (a, b) = (m("a", 3000), m("b", 3000));
        assert_eq!(
            r.plan(&a, None),
            Plan::Load {
                evict: vec![],
                cost_mib: 3000
            }
        );
        r.add("a", 2800);
        assert_eq!(r.plan(&a, None), Plan::Hit);
        assert_eq!(
            r.plan(&b, None),
            Plan::Load {
                evict: vec![],
                cost_mib: 3000
            }
        );
    }

    #[test]
    fn the_least_recently_used_model_goes_first() {
        let mut r = Residency::new(8000, 0, 8);
        r.add("a", 3000);
        r.add("b", 3000);
        r.touch("a"); // b is now the oldest
        let c = m("c", 3000);
        assert_eq!(
            r.plan(&c, None),
            Plan::Load {
                evict: vec!["b".into()],
                cost_mib: 3000
            }
        );
        // a bigger model needs both out, oldest first
        let big = m("big", 7000);
        assert_eq!(
            r.plan(&big, None),
            Plan::Load {
                evict: vec!["b".into(), "a".into()],
                cost_mib: 7000
            }
        );
    }

    #[test]
    fn the_resident_count_is_a_limit_too() {
        let mut r = Residency::new(100_000, 0, 2);
        r.add("a", 10);
        r.add("b", 10);
        assert_eq!(
            r.plan(&m("c", 10), None),
            Plan::Load {
                evict: vec!["a".into()],
                cost_mib: 10
            }
        );
    }

    #[test]
    fn a_model_over_the_budget_is_refused_not_loaded_by_evicting_everything() {
        let mut r = Residency::new(8000, 0, 4);
        r.add("a", 3000);
        assert_eq!(
            r.plan(&m("e4b", 9000), None),
            Plan::TooBig {
                need_mib: 9000,
                limit_mib: 8000
            }
        );
        assert!(r.is_resident("a"));
    }

    #[test]
    fn the_system_reserve_counts_as_well_as_the_budget() {
        let mut r = Residency::new(12_000, 2000, 4);
        r.add("a", 3000);
        // 5000 MiB available now; evicting a frees 3000 more; minus the reserve: 6000 usable.
        assert_eq!(
            r.plan(&m("b", 7000), Some(5000)),
            Plan::TooBig {
                need_mib: 7000,
                limit_mib: 6000
            }
        );
        // 5000 fits only by evicting a (5000 free - 2000 reserve = 3000 without evicting)
        assert_eq!(
            r.plan(&m("c", 5000), Some(5000)),
            Plan::Load {
                evict: vec!["a".into()],
                cost_mib: 5000
            }
        );
        assert_eq!(
            r.plan(&m("d", 2500), Some(5000)),
            Plan::Load {
                evict: vec![],
                cost_mib: 2500
            }
        );
    }

    #[test]
    fn a_load_that_costs_more_than_estimated_pushes_out_older_models() {
        let mut r = Residency::new(8000, 0, 4);
        r.add("old", 1200);
        r.add("new", 7300); // estimated 6000 when it was admitted
        assert_eq!(r.over_budget("new"), vec!["old".to_string()]);
        r.remove("old");
        assert!(
            r.over_budget("new").is_empty(),
            "the model just loaded is never the victim"
        );
        let mut r = Residency::new(8000, 0, 4);
        r.add("a", 1000);
        r.add("b", 9000);
        assert_eq!(r.over_budget("b"), vec!["a".to_string()]);
    }

    #[test]
    fn a_measured_cost_replaces_the_estimate_and_survives_eviction() {
        let mut r = Residency::new(8000, 0, 4);
        let a = m("a", 3000);
        r.add("a", 5200);
        r.remove("a");
        assert_eq!(r.cost_of(&a), 5200);
        assert_eq!(
            r.plan(&a, None),
            Plan::Load {
                evict: vec![],
                cost_mib: 5200
            }
        );
        r.seed_costs([("z".to_string(), 77)]);
        assert_eq!(r.cost_of(&m("z", 1)), 77);
    }
}
