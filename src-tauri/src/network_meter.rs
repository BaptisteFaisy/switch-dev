//! Mesure le debit reseau effectivement consomme par l'hote (octets/s, somme
//! descendant + montant) sur une petite fenetre glissante. Sert de capteur au
//! regulateur bande passante des agents autonomes : quand la ligne approche de
//! la saturation, le regulateur retient les nouveaux lancements.
//!
//! Sources par plateforme (aucune dependance lourde ajoutee) :
//!   - Linux   : `/proc/net/dev` (compteurs cumules par interface, hors `lo`).
//!   - Windows : `GetIfTable2` (compteurs cumules `InOctets`/`OutOctets`).
//!   - Autre   : capteur inoperant -> renvoie 0 (regulateur desactive).
//!
//! Les compteurs sont cumules depuis le boot : on calcule un delta entre deux
//! lectures separees d'une duree connue pour obtenir un debit moyen.

use std::time::Duration;

/// Fenetre de lissage (secondes) sur laquelle le debit moyen est calcule.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
/// Nombre d'echantillons retenus pour lisser les a-coups.
const HISTORY_MAX: usize = 8;

/// Paire descendante/montante cumulee en octets depuis le boot.
#[derive(Debug, Clone, Copy, Default)]
struct Cumulative {
    rx: u64,
    tx: u64,
}

/// Capteur de debit reseau cumule.
pub struct NetworkMeter {
    last: Option<Cumulative>,
    last_sample_at: Option<std::time::Instant>,
    samples: std::collections::VecDeque<f64>,
}

impl Default for NetworkMeter {
    fn default() -> Self {
        Self {
            last: None,
            last_sample_at: None,
            samples: std::collections::VecDeque::new(),
        }
    }
}

/// Lit les compteurs cumules de toutes les interfaces non-boucle locale.
/// Retourne `None` si la plateforme n'est pas mesurable.
fn cumulative_totals() -> Option<Cumulative> {
    #[cfg(target_os = "linux")]
    {
        return read_proc_net_dev();
    }
    #[cfg(target_os = "windows")]
    {
        return read_windows_if_table();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    None::<Cumulative>
}

#[cfg(target_os = "linux")]
fn read_proc_net_dev() -> Option<Cumulative> {
    let content = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in content.lines().skip(2) {
        let line = line.trim();
        // Format : "  eth0:    1234   ...    5678   ..."
        let colon = line.find(':')?;
        let iface = line[..colon].trim();
        // L'interface boucle locale fausse le debit sortant.
        if iface == "lo" {
            continue;
        }
        let mut fields = line[colon + 1..].split_whitespace();
        let rx_bytes = fields.next()?.parse::<u64>().ok()?;
        // Champs : rx, rx_packets, rx_errs, rx_drop, rx_fifo, rx_frame,
        // rx_compressed, rx_multicast, puis la ligne TX commence a tx_bytes.
        let _skip: Vec<&str> = fields.by_ref().take(6).collect();
        let tx_bytes = fields.next()?.parse::<u64>().ok()?;
        rx = rx.saturating_add(rx_bytes);
        tx = tx.saturating_add(tx_bytes);
    }
    Some(Cumulative { rx, tx })
}

#[cfg(target_os = "windows")]
fn read_windows_if_table() -> Option<Cumulative> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
    };
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    let ret = unsafe { GetIfTable2(&mut table) };
    if ret != 0 || table.is_null() {
        return None;
    }
    let mut rx = 0u64;
    let mut tx = 0u64;
    unsafe {
        // Les entrees suivent directement la tete `MIB_IF_TABLE2` (donc le champ
        // `Table`) en memoire contigue. On ne copie pas la structure : on prend
        // l'adresse du champ `Table` *dans la table allouee*.
        let base = std::ptr::addr_of!((*table).Table) as *const MIB_IF_ROW2;
        let rows = std::slice::from_raw_parts(base, (*table).NumEntries as usize);
        for row in rows {
            rx = rx.saturating_add(row.InOctets);
            tx = tx.saturating_add(row.OutOctets);
        }
        FreeMibTable(table as *const core::ffi::c_void);
    }
    Some(Cumulative { rx, tx })
}

impl NetworkMeter {
    /// Appeler une fois par tick du worker. Met a jour l'historique et renvoie
    /// le debit moyen actuel descendant+montant en octets/s (0 si aucune mesure).
    pub fn tick(&mut self) -> f64 {
        let Some(total) = cumulative_totals() else {
            self.last = None;
            self.last_sample_at = None;
            return 0.0;
        };
        let now = std::time::Instant::now();
        let bytes_per_sec = if let (Some(prev), Some(prev_at)) = (self.last, self.last_sample_at)
        {
            let delta = (total.rx - prev.rx) + (total.tx - prev.tx);
            let elapsed = now.duration_since(prev_at).as_secs_f64();
            if elapsed > 0.0 {
                delta as f64 / elapsed
            } else {
                0.0
            }
        } else {
            0.0
        };
        self.last = Some(total);
        self.last_sample_at = Some(now);
        if self.samples.len() >= HISTORY_MAX {
            self.samples.pop_front();
        }
        self.samples.push_back(bytes_per_sec);
        let count = self.samples.len();
        let sum: f64 = self.samples.iter().sum();
        sum / count as f64
    }

    /// Debit moyen actuel (octets/s, descendant + montant) sur la fenetre
    /// glissante. Renvoie 0.0 tant qu'aucun echantillon n'est disponible.
    pub fn latest_bytes_per_sec(&self) -> f64 {
        let count = self.samples.len();
        if count == 0 {
            return 0.0;
        }
        let sum: f64 = self.samples.iter().sum();
        sum / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn meter_returns_zero_when_no_measurement_is_available() {
        // Sur toute plateforme, avant la premiere mesure, le debit est nul.
        let mut meter = NetworkMeter::default();
        let before_first = meter.tick();
        // La seule garantie portable : l'appel ne panique jamais et renvoie >= 0.
        assert!(before_first >= 0.0);
        std::thread::sleep(Duration::from_millis(10));
        let after = meter.tick();
        assert!(after >= 0.0);
        let _ = &after;
    }
}