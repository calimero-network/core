//! `Text` costs: what one edit or read asks of the host, pinned with `MemStore`'s counters at
//! 100, 1,000 and 10,000 runs, so a change that brings back whole-document edits fails here.

mod costs {
    use calimero_sdk::{MemStore, StorageError, Text};

    const SIZES: [u64; 3] = [100, 1_000, 10_000];

    /// A text of `n` characters, each typed at the start: `n` runs of one character, the
    /// most fragmented a text of `n` characters can be.
    fn fragmented(n: u64) -> (Text<MemStore>, MemStore) {
        let store = MemStore::new();
        let mut t = Text::with_store("doc", store.clone());
        for i in 0..n {
            let ch = char::from(b'a' + (i % 26) as u8);
            let _ = t.insert(0, &ch.to_string()).unwrap();
        }
        store.reset_costs();
        (t, store)
    }

    /// (calls, writes, bytes written, bytes read) since the last reset.
    fn costs(store: &MemStore) -> (usize, usize, usize, usize) {
        let c = (
            store.calls(),
            store.writes(),
            store.written_bytes(),
            store.read_bytes(),
        );
        store.reset_costs();
        c
    }

    #[test]
    fn typing_costs_the_same_at_any_length() {
        for n in SIZES {
            let (mut t, store) = fragmented(n);
            let mid = t.id_at(n / 2).unwrap().unwrap();
            store.reset_costs();
            // A keystroke after a character in the middle: a new run beside it.
            let typed = t.insert_after(Some(mid), "x").unwrap()[0];
            let first = costs(&store);
            // The next keystroke after it: the run grows.
            let _ = t.insert_after(Some(typed), "y").unwrap();
            let next = costs(&store);
            // Backspace on it.
            let _ = t.delete_ids(&[typed]).unwrap();
            let back = costs(&store);
            eprintln!("typing n={n}: first {first:?}, next {next:?}, backspace {back:?}");
            for (calls, writes, written, read) in [first, next, back] {
                assert!(calls <= 18, "n={n}: {calls} calls");
                assert!(writes <= 8, "n={n}: {writes} writes");
                assert!(written <= 2_500, "n={n}: {written} bytes written");
                assert!(read <= 3_000, "n={n}: {read} bytes read");
            }
        }
    }

    #[test]
    fn a_ten_thousand_character_paste_is_a_handful_of_writes() {
        let paste = "p".repeat(10_000);
        for n in SIZES {
            let (mut t, store) = fragmented(n);
            let ids = t.insert(n / 2, &paste).unwrap();
            let (calls, writes, written, read) = costs(&store);
            eprintln!(
                "paste n={n}: {calls} calls, {writes} writes, {written} B written, {read} B read"
            );
            assert_eq!(ids.len(), 10_000);
            assert!(writes <= 16, "n={n}: {writes} writes");
            assert!(written <= 12_000, "n={n}: {written} bytes");
            assert_eq!(t.len().unwrap(), n + 10_000);

            // Deleting the pasted range touches its five runs.
            let at = t.index_of(ids[0]).unwrap().unwrap();
            store.reset_costs();
            assert_eq!(t.delete(at, 10_000).unwrap(), 10_000);
            let (calls, writes, written, read) = costs(&store);
            eprintln!(
                "delete n={n}: {calls} calls, {writes} writes, {written} B written, {read} B read"
            );
            assert!(writes <= 12, "n={n}: {writes} writes");
            assert_eq!(t.len().unwrap(), n);

            // And by ids, the way an editor deletes a selection.
            let again = t.insert(0, &paste).unwrap();
            store.reset_costs();
            assert_eq!(t.delete_ids(&again).unwrap(), 10_000);
            let (calls, writes, ..) = costs(&store);
            eprintln!("delete_ids n={n}: {calls} calls, {writes} writes");
            assert!(writes <= 12, "n={n}: {writes} writes");
        }
    }

    #[test]
    fn index_lookups_read_one_path() {
        for n in SIZES {
            let (t, store) = fragmented(n);
            let id = t.id_at(n / 3).unwrap().unwrap();
            let at = costs(&store);
            assert_eq!(t.index_of(id).unwrap(), Some(n / 3));
            let of = costs(&store);
            eprintln!("id_at n={n}: {at:?}; index_of: {of:?}");
            assert!(at.0 <= 6 && of.0 <= 8, "n={n}: {at:?} {of:?}");
            assert!(at.3 <= 3_000 && of.3 <= 3_000, "n={n}: {at:?} {of:?}");
        }
    }

    #[test]
    fn reading_costs_the_live_text_and_its_runs() {
        for n in SIZES {
            // Fragmented: n runs of one character.
            let (t, store) = fragmented(n);
            assert_eq!(t.read().unwrap().len() as u64, n);
            let (calls, _, _, read) = costs(&store);
            eprintln!("read fragmented n={n}: {calls} calls, {read} B");
            assert!(read as u64 <= 60 * n + 100, "n={n}: {read} bytes");

            // Typed in order: runs of 512.
            let store = MemStore::new();
            let mut t = Text::with_store("doc", store.clone());
            let mut last = None;
            for _ in 0..n {
                last = t.insert_after(last, "z").unwrap().pop();
            }
            store.reset_costs();
            assert_eq!(t.read().unwrap().len() as u64, n);
            let (calls, _, _, read) = costs(&store);
            eprintln!("read typed n={n}: {calls} calls, {read} B");
            assert!(read as u64 <= n + 200 + n / 5, "n={n}: {read} bytes");
            // Every keystroke grew one run: one write for the run, its leaf, the counts.
            store.reset_costs();
            let _ = t.insert_after(last, "z").unwrap();
            let (calls, writes, written, _) = costs(&store);
            eprintln!("typing at the end n={n}: {calls} calls, {writes} writes, {written} B");
            assert!(writes <= 5, "n={n}: {writes} writes");
        }
    }

    #[test]
    fn a_paste_past_the_run_limits_is_refused_up_front() {
        let store = MemStore::new();
        let mut t = Text::with_store("doc", store.clone());
        let too_much = "é".repeat(600_000);
        let err = t.insert(0, &too_much).unwrap_err();
        assert!(matches!(err, StorageError::EditTooLarge { .. }), "{err}");
        assert_eq!(store.writes(), 0);
        assert_eq!(t.len().unwrap(), 0);
    }
}
