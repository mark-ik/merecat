//! Shared row geometry for the list panes (Trail, Roster, ...). Rung 5 slice D.
//!
//! A list pane renders rows at a fixed height and hit-tests a pointer back to a
//! row by the same numbers, so what the pointer hits is exactly what it sees.
//! Both the renderer (row y-positions) and the click router (window-y to index)
//! read these, so the two never drift.

/// Height of one row, in physical pixels.
pub const ROW_HEIGHT: f32 = 26.0;
/// Inset above the first row.
pub const TOP_INSET: f32 = 8.0;

/// The row a point at pane-local `y` falls on, given `len` rows, or `None` above
/// the first row or below the last.
pub fn row_index_at(len: usize, local_y: f32) -> Option<usize> {
    if local_y < TOP_INSET {
        return None;
    }
    let idx = ((local_y - TOP_INSET) / ROW_HEIGHT).floor() as usize;
    (idx < len).then_some(idx)
}

/// The y-position of row `i`.
pub fn row_y(i: usize) -> f32 {
    TOP_INSET + i as f32 * ROW_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_index_round_trips_with_row_y() {
        for i in 0..5 {
            // A point just inside row i maps back to i.
            assert_eq!(row_index_at(5, row_y(i) + 1.0), Some(i));
        }
        assert_eq!(row_index_at(5, 2.0), None); // above the first row
        assert_eq!(row_index_at(5, row_y(5) + 1.0), None); // below the last
    }
}
