//! Colors for people.
//!
//! Each person gets a pill color that is distinct from everyone else on the
//! board. Names are hashed to a preferred slot, and collisions move to the
//! next free slot — so colors are stable across runs while the team is, and
//! two people never share one (up to the palette size). The palette is vivid
//! hues spread around the wheel, then lighter versions of them; reds, oranges
//! and browns are left out so pills never read as the red "Unassigned" pill or
//! the orange cursor accent.

use std::collections::HashMap;

use ratatui::style::Color;

/// 11 hues (55°–330°, skipping red/orange/brown) at full strength, then lighter.
const PALETTE: [Color; 22] = [
    Color::Rgb(221, 209, 75),
    Color::Rgb(148, 221, 75),
    Color::Rgb(75, 221, 99),
    Color::Rgb(75, 221, 184),
    Color::Rgb(75, 196, 221),
    Color::Rgb(75, 148, 221),
    Color::Rgb(75, 87, 221),
    Color::Rgb(124, 75, 221),
    Color::Rgb(184, 75, 221),
    Color::Rgb(221, 75, 196),
    Color::Rgb(221, 75, 148),
    Color::Rgb(240, 232, 148),
    Color::Rgb(194, 240, 148),
    Color::Rgb(148, 240, 163),
    Color::Rgb(148, 240, 217),
    Color::Rgb(148, 224, 240),
    Color::Rgb(148, 194, 240),
    Color::Rgb(148, 156, 240),
    Color::Rgb(179, 148, 240),
    Color::Rgb(217, 148, 240),
    Color::Rgb(240, 148, 224),
    Color::Rgb(240, 148, 194),
];
const HUES: usize = 11;

#[derive(Debug, Default)]
pub struct UserColors {
    slots: HashMap<String, usize>,
}

impl UserColors {
    /// Give everyone in `names` a slot. Names are handled in sorted order so
    /// the result doesn't depend on board order; people who already have a
    /// color keep it.
    pub fn assign<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        let mut names: Vec<&str> = names.into_iter().filter(|n| !self.slots.contains_key(*n)).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            let taken: Vec<usize> = self.slots.values().copied().collect();
            let preferred = (hash(name) % HUES as u64) as usize;
            // Probe the vivid ring from the preferred hue, then the light ring.
            let slot = (0..PALETTE.len())
                .map(|i| (i / HUES) * HUES + (preferred + i) % HUES)
                .find(|slot| !taken.contains(slot))
                .unwrap_or(preferred);
            self.slots.insert(name.to_string(), slot);
        }
    }

    /// The person's color (a hashed one if they were never assigned).
    pub fn color(&self, name: &str) -> Color {
        let slot = self
            .slots
            .get(name)
            .copied()
            .unwrap_or_else(|| (hash(name) % PALETTE.len() as u64) as usize);
        PALETTE[slot]
    }
}

/// A stable color for any string (e.g. an epic without a Jira color).
pub fn hashed(text: &str) -> Color {
    PALETTE[(hash(text) % HUES as u64) as usize]
}

/// FNV-1a: unlike std's hasher, guaranteed stable across Rust releases.
fn hash(text: &str) -> u64 {
    text.bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everyone_on_the_board_gets_a_distinct_color() {
        let names = [
            "Riley Chen", "Jamie Park", "Noor Haddad", "Sam Okafor (Contractor)", "Lee Wong",
            "Priya Nair", "Alex Kim", "Morgan Hale", "Venkata Rao Subramanian",
            "Diego Alvarez (Contractor)", "Marta Rossi (Contractor)", "Avery Brooks", "Quinn Murphy",
        ];
        let mut colors = UserColors::default();
        colors.assign(names);
        let distinct: std::collections::HashSet<String> =
            names.iter().map(|n| format!("{:?}", colors.color(n))).collect();
        assert_eq!(distinct.len(), names.len());
    }

    #[test]
    fn assignment_is_stable_and_independent_of_order() {
        let mut a = UserColors::default();
        a.assign(["Ann", "Bob", "Cy"]);
        let mut b = UserColors::default();
        b.assign(["Cy", "Ann", "Bob"]);
        for name in ["Ann", "Bob", "Cy"] {
            assert_eq!(a.color(name), b.color(name));
        }
    }

    #[test]
    fn existing_people_keep_their_color_when_someone_joins() {
        let mut colors = UserColors::default();
        colors.assign(["Ann", "Bob"]);
        let before = (colors.color("Ann"), colors.color("Bob"));
        colors.assign(["Zed"]);
        assert_eq!(before, (colors.color("Ann"), colors.color("Bob")));
        assert_ne!(colors.color("Zed"), colors.color("Ann"));
        assert_ne!(colors.color("Zed"), colors.color("Bob"));
    }

    #[test]
    fn the_palette_has_no_reds_oranges_or_browns() {
        for color in PALETTE {
            let Color::Rgb(r, g, b) = color else { panic!("rgb only") };
            let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
            let (max, min) = (r.max(g).max(b), r.min(g).min(b));
            if max - min < 1.0 {
                continue;
            }
            let hue = if max == r {
                60.0 * ((g - b) / (max - min)).rem_euclid(6.0)
            } else if max == g {
                60.0 * ((b - r) / (max - min) + 2.0)
            } else {
                60.0 * ((r - g) / (max - min) + 4.0)
            };
            assert!(!(hue < 50.0 || hue > 340.0), "hue {hue} is red/orange/brown");
        }
    }
}
