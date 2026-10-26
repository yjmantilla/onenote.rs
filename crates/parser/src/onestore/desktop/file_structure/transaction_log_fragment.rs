use super::super::common::FileChunkReference64x32;
use crate::Reader;
use crate::errors::{ErrorKind, Result};
use crate::onestore::desktop::parse::Parse;
use onenote_parser_macros::Parse;

/// See [\[MS-ONESTORE\] 2.3.3.1](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-onestore/158030a2-dbf0-4b92-bf6e-1a91a403aebd)
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct TransactionLogFragment {
    pub(crate) size_table: Vec<TransactionEntry>,
    pub(crate) next_fragment: FileChunkReference64x32,
    /// Number of complete (sentinel-terminated) transactions read from this fragment.
    pub(crate) transaction_count: u32,
}

impl TransactionLogFragment {
    /// Parses a fragment, reading at most `max_transactions` transactions.
    ///
    /// Only the first `cTransactionsInLog` transactions of the log are committed
    /// (see [\[MS-ONESTORE\] 2.3.1]). The space after the last committed transaction,
    /// including the `nextFragment` field of the last fragment, can hold stale data
    /// from earlier writes and must not be interpreted.
    ///
    /// [\[MS-ONESTORE\] 2.3.1]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-onestore/2b394c6b-8788-441f-b631-da1583d772fd
    pub(crate) fn parse(reader: Reader, size: usize, max_transactions: u32) -> Result<Self> {
        if size < 12 {
            return Err(ErrorKind::MalformedOneStoreData(
                format!("TransactionLogFragment too small: {size} bytes").into(),
            )
            .into());
        }

        let size_table_count = (size - 12) / 8;
        let mut size_table = Vec::new();
        let mut pending = Vec::new();
        let mut transaction_count = 0;
        for _i in 0..size_table_count {
            if transaction_count == max_transactions {
                break;
            }

            let entry = TransactionEntry::parse(reader)?;
            if entry.is_sentinel() {
                transaction_count += 1;
                size_table.append(&mut pending);
            } else {
                pending.push(entry);
            }
        }

        if transaction_count == max_transactions {
            // The log is complete; whatever follows (including nextFragment) is unused space.
            return Ok(Self {
                size_table,
                next_fragment: FileChunkReference64x32 {
                    stp: u64::MAX,
                    cb: 0,
                },
                transaction_count,
            });
        }

        // According to \[MS-ONESTORE\] 2.3.3.1, a fragment that doesn't complete the log
        // ends with a sentinel entry, so no partial transaction may remain.
        if !pending.is_empty() {
            return Err(ErrorKind::MalformedOneStoreData(
                format!(
                    "The size_table must end in a sentinel entry. Total entries: {}. Last entry: {:?}",
                    size_table_count,
                    pending.last()
                )
                .into(),
            )
            .into());
        }

        Ok(Self {
            size_table,
            next_fragment: FileChunkReference64x32::parse(reader)?,
            transaction_count,
        })
    }
}

/// See [\[MS-ONESTORE\] 2.3.3.2](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-onestore/c00897d9-d90a-4707-b9fb-58c93e490322)
#[derive(Debug, Parse)]
#[allow(dead_code)]
pub(crate) struct TransactionEntry {
    pub(crate) src_id: u32,
    pub(crate) transaction_entry_switch: u32,
}

impl TransactionEntry {
    pub(crate) fn is_sentinel(&self) -> bool {
        self.src_id == 0x00000001
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onestore::desktop::common::FileChunkReference;
    use crate::reader::Reader;

    fn entry(src_id: u32, switch: u32) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&src_id.to_le_bytes());
        bytes[4..].copy_from_slice(&switch.to_le_bytes());
        bytes
    }

    fn fragment(entries: &[[u8; 8]], next_stp: u64, next_cb: u32) -> Vec<u8> {
        let mut data: Vec<u8> = entries.iter().flatten().copied().collect();
        data.extend_from_slice(&next_stp.to_le_bytes());
        data.extend_from_slice(&next_cb.to_le_bytes());
        data
    }

    #[test]
    fn ignores_stale_entries_after_last_committed_transaction() {
        // One committed transaction, followed by stale data and a garbage nextFragment.
        let data = fragment(
            &[
                entry(0x20, 3),
                entry(1, 0),
                entry(0x55, 999),
                entry(0x66, 7),
            ],
            0x5554_3EA3_8E39_40EC,
            30_359_125,
        );
        let mut reader = Reader::new(&data);
        let parsed = TransactionLogFragment::parse(&mut reader, data.len(), 1).unwrap();

        assert_eq!(parsed.transaction_count, 1);
        assert_eq!(parsed.size_table.len(), 1);
        assert_eq!(parsed.size_table[0].src_id, 0x20);
        assert_eq!(parsed.size_table[0].transaction_entry_switch, 3);
        assert!(parsed.next_fragment.is_fcr_nil());
    }

    #[test]
    fn follows_next_fragment_while_transactions_remain() {
        let data = fragment(&[entry(0x20, 3), entry(1, 0)], 0x400, 0x40);
        let mut reader = Reader::new(&data);
        let parsed = TransactionLogFragment::parse(&mut reader, data.len(), 2).unwrap();

        assert_eq!(parsed.transaction_count, 1);
        assert_eq!(parsed.next_fragment.stp, 0x400);
        assert_eq!(parsed.next_fragment.cb, 0x40);
    }

    #[test]
    fn rejects_partial_transaction_in_incomplete_log() {
        let data = fragment(&[entry(0x20, 3), entry(1, 0), entry(0x30, 1)], 0x400, 0x40);
        let mut reader = Reader::new(&data);

        assert!(TransactionLogFragment::parse(&mut reader, data.len(), 5).is_err());
    }
}
