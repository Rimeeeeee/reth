use alloc::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, StorageKey, StorageValue, TxIndex};
use revm::{
    bytecode::opcode,
    context::ContextTr,
    inspector::JournalExt,
    interpreter::{
        interpreter_types::{InputsTr, Jumps},
        Interpreter,
    },
    Database, Inspector,
};

/// Alias for `storage_read` in `StorageChangeInspector`.
pub type StorageReads = BTreeMap<Address, BTreeSet<StorageKey>>;
/// Alias for `storage_write` in `StorageChangeInspector`.
pub type StorageWrites = BTreeMap<Address, BTreeMap<StorageKey, (StorageValue, StorageValue)>>;

/// An Inspector that tracks warm and cold storage slot accesses.
#[derive(Debug, Clone, Default)]
pub struct StorageChangeInspector {
    /// Tracks reads (SLOAD)
    pub storage_read: StorageReads,
    /// Tracks writes (SSTORE): address → slot → (pre, post)
    pub storage_write: StorageWrites,
    /// Current transaction index
    pub tx_index: TxIndex,
}

impl StorageChangeInspector {
    /// Creates a new inspector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the current transaction index.
    pub const fn set_tx_index(&mut self, index: TxIndex) {
        self.tx_index = index;
    }

    /// Resets storage read and write.
    pub fn reset(&mut self) {
        self.storage_read.clear();
        self.storage_write.clear();
        self.tx_index = TxIndex::default();
    }

    /// Slots that were only read (SLOAD) but not written (SSTORE)
    pub fn read_only_slots(&self) -> StorageReads {
        let mut result: StorageReads = BTreeMap::new();
        for (addr, read_slots) in &self.storage_read {
            let written = self
                .storage_write
                .get(addr)
                .map(|w| w.keys().copied().collect::<BTreeSet<_>>())
                .unwrap_or_default();

            let read_only = read_slots.difference(&written).copied().collect();
            result.insert(*addr, read_only);
        }
        result
    }

    /// Slots written with same value as pre (no-op SSTOREs)
    pub fn unchanged_writes(&self) -> StorageReads {
        let mut result: StorageReads = BTreeMap::new();
        for (addr, slots) in &self.storage_write {
            for (slot, (pre, post)) in slots {
                if pre == post {
                    result.entry(*addr).or_default().insert(*slot);
                }
            }
        }
        result
    }

    /// Returns all "read" slots (did not result in state change)
    pub fn get_bal_storage_reads(&self) -> StorageReads {
        let mut result: StorageReads = BTreeMap::new();

        for (addr, slots) in self.read_only_slots() {
            result.entry(addr).or_default().extend(slots);
        }
        for (addr, slots) in self.unchanged_writes() {
            result.entry(addr).or_default().extend(slots);
        }

        result
    }

    /// Returns all storage writes that changed the state
    pub fn get_bal_storage_writes(&self) -> StorageWrites {
        let mut result: StorageWrites = BTreeMap::new();

        for (addr, slots) in &self.storage_write {
            for (slot, (pre, post)) in slots {
                if pre != post || (*pre != StorageValue::ZERO && *post == StorageValue::ZERO) {
                    result.entry(*addr).or_default().insert(*slot, (*pre, *post));
                }
            }
        }

        result
    }

    /// Returns all storage writes that changed the state.
    pub const fn reads(&self) -> &StorageReads {
        &self.storage_read
    }

    /// Returns all storage writes that changed the state.
    pub const fn writes(&self) -> &StorageWrites {
        &self.storage_write
    }
}

impl<CTX> Inspector<CTX> for StorageChangeInspector
where
    CTX: ContextTr<Journal: JournalExt>,
{
    fn step(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        let opcode = interp.bytecode.opcode();
        let address = interp.input.target_address();
        let journal = context.journal_ref();
        match opcode {
            opcode::SLOAD => {
                if let Ok(slot) = interp.stack.peek(0) {
                    let key = StorageKey::from(slot.to_be_bytes());
                    self.storage_read.entry(address).or_default().insert(key);

                    for entry in journal.journal().iter().rev() {
                        if let revm::JournalEntry::StorageChanged {
                            address: changed_addr,
                            key: slot_key,
                            had_value,
                        } = entry
                        {
                            if *changed_addr == address && *slot_key == key.into() {
                                let post = journal.evm_state()[changed_addr].storage[slot_key]
                                    .present_value();
                                self.storage_write
                                    .entry(address)
                                    .or_default()
                                    .entry(key)
                                    .or_insert((*had_value, post));
                                break;
                            }
                        }
                    }
                }
            }
            opcode::SSTORE => {
                if let Ok(slot) = interp.stack.peek(0) {
                    let key = StorageKey::from(slot.to_be_bytes());

                    // Load pre-value from existing recorded value or DB
                    let pre = self
                        .storage_write
                        .get(&address)
                        .and_then(|m| m.get(&key).map(|(pre, _)| *pre))
                        .unwrap_or_else(|| {
                            context.db_mut().storage(address, slot).unwrap_or_default()
                        });

                    // Get current top of stack (new value to be stored)
                    let post = interp.stack.peek(1).unwrap_or(StorageValue::ZERO);

                    // Update collapsed (final) view of SSTORE
                    self.storage_write.entry(address).or_default().insert(key, (pre, post));
                }
            }
            _ => {}
        }
    }
}
