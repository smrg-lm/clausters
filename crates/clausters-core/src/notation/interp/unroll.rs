//! **The order the measures are played in**: the written order, with its
//! repeats, endings and jumps played out.
//!
//! A page is read in one order and played in another. A repeat sign sends the
//! player back to where the repeat starts (the score's start, or the last
//! start-repeat sign), once; an ending is played only in the passes its label
//! names; a *da capo* goes back to the start and a *dal segno* to the sign, and
//! after the jump the repeats are not taken again, the last ending is the one
//! played, a *fine* ends the piece and a *to coda* goes to the coda -- the
//! conventions every player reads with.

use crate::notation::model::Grid;
use crate::ratio::Ratio;

/// One measure as it is played: which measure, and where its pass starts in
/// the performance, in whole notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Played {
    pub measure: usize,
    pub at: Ratio,
}

/// The passes an ending's label names: `"1"`, `"2"`, `"1, 2"`, `"1-3"`.
fn passes(label: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in label.split([',', ' ', '.']).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                    out.extend(a..=b);
                }
            }
            None => out.extend(part.trim().parse::<usize>().ok()),
        }
    }
    out
}

/// **The measures of a score `count` measures long, as they are played.**
pub fn unroll(grid: &Grid, count: usize) -> Vec<Played> {
    let barline = |m: usize| {
        grid.barlines
            .iter()
            .find(|(b, _)| *b == m)
            .map(|(_, kind)| kind.as_str())
    };
    let mark = |m: usize, kind: &str| grid.marks.iter().any(|(b, k)| *b == m && k == kind);
    let sign = |kind: &str| grid.marks.iter().find(|(_, k)| k == kind).map(|(m, _)| *m);

    let mut out: Vec<Played> = Vec::new();
    let mut at = Ratio::ZERO;
    let mut m = 0;
    let mut start = 0;
    let mut pass = 1;
    let mut jumped = false;
    // a score that jumps forever is a written mistake, and a bounded one
    let limit = count.saturating_mul(16).max(16);
    while m < count && out.len() < limit {
        if let Some((_, last, label)) = grid.endings.iter().find(|(a, _, _)| *a == m) {
            let played = passes(label);
            // after a jump the pass is the second: the last ending is played
            let take = played.is_empty() || played.contains(&pass);
            if !take {
                m = last + 1;
                continue;
            }
        }
        out.push(Played { measure: m, at });
        at = at + grid.bar_len(m);
        let right = barline(m);
        if matches!(right, Some("rptend" | "rptboth")) && !jumped {
            // the furthest pass an ending over this stretch names
            let most = grid
                .endings
                .iter()
                .filter(|(a, _, _)| *a >= start && *a <= m + 1)
                .flat_map(|(_, _, label)| passes(label))
                .max()
                .unwrap_or(2)
                .max(2);
            if pass < most {
                pass += 1;
                m = start;
                continue;
            }
            pass = 1;
        }
        if matches!(right, Some("rptstart" | "rptboth")) {
            start = m + 1;
            if right == Some("rptstart") {
                pass = 1;
            }
        }
        if jumped && mark(m, "fine") {
            break;
        }
        if jumped
            && mark(m, "tocoda")
            && let Some(coda) = grid
                .marks
                .iter()
                .find(|(b, k)| k == "coda" && *b > m)
                .map(|(b, _)| *b)
        {
            m = coda;
            continue;
        }
        if !jumped && mark(m, "dacapo") {
            jumped = true;
            pass = 2;
            m = 0;
            start = 0;
            continue;
        }
        if !jumped
            && mark(m, "dalsegno")
            && let Some(segno) = sign("segno")
        {
            jumped = true;
            pass = 2;
            m = segno;
            start = segno;
            continue;
        }
        m += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(grid: &Grid, count: usize) -> Vec<usize> {
        unroll(grid, count).iter().map(|p| p.measure).collect()
    }

    #[test]
    fn a_score_with_no_signs_is_played_as_written() {
        assert_eq!(order(&Grid::default(), 3), vec![0, 1, 2]);
    }

    #[test]
    fn a_repeat_goes_back_once_and_endings_take_their_pass() {
        let mut grid = Grid {
            barlines: vec![(1, "rptend".into())],
            ..Grid::default()
        };
        assert_eq!(order(&grid, 3), vec![0, 1, 0, 1, 2]);
        // a start sign moves where it goes back to
        grid.barlines = vec![(0, "rptstart".into()), (2, "rptend".into())];
        assert_eq!(order(&grid, 4), vec![0, 1, 2, 1, 2, 3]);
        // first and second endings
        grid.barlines = vec![(1, "rptend".into())];
        grid.endings = vec![(1, 1, "1".into()), (2, 2, "2".into())];
        assert_eq!(order(&grid, 4), vec![0, 1, 0, 2, 3]);
    }

    #[test]
    fn a_jump_plays_back_to_its_sign_and_stops_at_fine() {
        let mut grid = Grid {
            marks: vec![(1, "fine".into()), (3, "dacapo".into())],
            ..Grid::default()
        };
        assert_eq!(order(&grid, 4), vec![0, 1, 2, 3, 0, 1]);
        grid.marks = vec![(1, "segno".into()), (3, "dalsegno".into())];
        assert_eq!(order(&grid, 5), vec![0, 1, 2, 3, 1, 2, 3, 4]);
        // to the coda from its sign, after the jump
        grid.marks = vec![
            (1, "tocoda".into()),
            (2, "dacapo".into()),
            (3, "coda".into()),
        ];
        assert_eq!(order(&grid, 4), vec![0, 1, 2, 0, 1, 3]);
        // repeats are not taken after a jump
        grid.marks = vec![(2, "dacapo".into())];
        grid.barlines = vec![(0, "rptend".into())];
        assert_eq!(order(&grid, 3), vec![0, 0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn the_passes_are_read_as_a_reader_writes_them() {
        assert_eq!(passes("1"), vec![1]);
        assert_eq!(passes("1, 2"), vec![1, 2]);
        assert_eq!(passes("1-3"), vec![1, 2, 3]);
    }
}
