//! Disponibilité de chaque pièce dans l'essaim (ajouté pour Pocket Torrent, §4.1) : combien de
//! pairs connectés l'ont. Sert au choix « les plus rares d'abord » (rarest-first) : une pièce
//! que peu de pairs possèdent risque de disparaître avec eux, on la prend tant qu'elle est là.
//!
//! Tenu à jour quand un pair annonce ses pièces (bitfield), en gagne une (have) ou se
//! déconnecte. Compteurs atomiques : mis à jour depuis la tâche de chaque pair, sans verrou.

use std::sync::atomic::{AtomicU16, Ordering};

use crate::type_aliases::BF;

pub(crate) struct PieceAvailability {
    counts: Box<[AtomicU16]>,
}

impl PieceAvailability {
    pub fn new(total_pieces: usize) -> Self {
        Self {
            counts: (0..total_pieces).map(|_| AtomicU16::new(0)).collect(),
        }
    }

    pub fn get(&self, piece: usize) -> u16 {
        self.counts
            .get(piece)
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    pub fn add_piece(&self, piece: usize) {
        if let Some(c) = self.counts.get(piece) {
            let _ = c.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_add(1))
            });
        }
    }

    fn remove_piece(&self, piece: usize) {
        if let Some(c) = self.counts.get(piece) {
            let _ = c.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        }
    }

    /// Pièces d'un bitfield (les bits de remplissage au-delà de la dernière pièce sont ignorés).
    fn ones<'a>(&'a self, bitfield: &'a BF) -> impl Iterator<Item = usize> + 'a {
        bitfield.iter_ones().take_while(|&i| i < self.counts.len())
    }

    pub fn add_bitfield(&self, bitfield: &BF) {
        for piece in self.ones(bitfield) {
            self.add_piece(piece);
        }
    }

    pub fn remove_bitfield(&self, bitfield: &BF) {
        for piece in self.ones(bitfield) {
            self.remove_piece(piece);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bf(bits: &[bool]) -> BF {
        let mut bf = BF::from_boxed_slice(vec![0u8; 2].into_boxed_slice());
        for (i, b) in bits.iter().enumerate() {
            bf.set(i, *b);
        }
        bf
    }

    #[test]
    fn counts_follow_peers() {
        let a = PieceAvailability::new(4);
        let p1 = bf(&[true, true, false, false]);
        let p2 = bf(&[true, false, false, true]);
        a.add_bitfield(&p1);
        a.add_bitfield(&p2);
        a.add_piece(2); // « have » reçu d'un pair
        assert_eq!(
            (0..4).map(|i| a.get(i)).collect::<Vec<_>>(),
            vec![2, 1, 1, 1]
        );
        a.remove_bitfield(&p1); // déconnexion
        assert_eq!(
            (0..4).map(|i| a.get(i)).collect::<Vec<_>>(),
            vec![1, 0, 1, 1]
        );
    }

    #[test]
    fn padding_bits_and_underflow_are_harmless() {
        let a = PieceAvailability::new(3);
        a.add_bitfield(&bf(&[false, false, false, false, false, true, true])); // bits de remplissage
        assert_eq!(a.get(2), 0);
        a.remove_bitfield(&bf(&[true, true, true]));
        assert_eq!(a.get(0), 0, "jamais négatif");
        assert_eq!(a.get(99), 0, "hors limites : 0");
    }
}
