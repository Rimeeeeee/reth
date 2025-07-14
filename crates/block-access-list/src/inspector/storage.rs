use alloy_primitives::{
    map::{foldhash::HashSet, HashMap},
    Address, StorageKey, StorageValue, TxIndex,
};
use revm::{
    bytecode::opcode,
    context::ContextTr,
    inspector::JournalExt,
    interpreter::{
        interpreter_types::{InputsTr, Jumps},
        Interpreter,
    },
    Inspector,
};

/// May need edgecases as per Eip and also needs Ordering
/// TODO! Use maube BTreeMap or similar to keep insertion order

/// An Inspector that tracks warm and cold storage slot accesses.
#[derive(Debug, Clone, Default)]
pub struct StorageChangeInspector {
    /// Tracks reads (SLOAD)
    pub storage_read: BTreeMap<Address, BTreeSet<StorageKey>>,
    /// Tracks writes (SSTORE): address → slot → (pre, post)
    pub storage_write: BTreeMap<Address, BTreeMap<StorageKey, (StorageValue, StorageValue)>>,
    /// Current transaction index
    pub tx_index: TxIndex,
}

impl StorageChangeInspector {
    /// Initializes the storage change inspector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the transaction index for the current transaction.
    pub fn set_tx_index(&mut self, index: TxIndex) {
        self.tx_index = index;
    }

    /// Sets the pre-state for storage slots.
    pub fn set_pre_state(&mut self, pre: HashMap<Address, HashMap<StorageKey, StorageValue>>) {
        self.pre_state = pre;
    }

    /// Resets the inspector state, clearing all tracked storage reads and writes.
    pub fn reset(&mut self) {
        self.storage_read.clear();
        self.storage_write.clear();
    }

    /// Returns the transaction index of the current transaction.
    pub fn reads(&self) -> &HashMap<Address, HashSet<StorageKey>> {
        &self.storage_read
    }

    /// Returns the storage writes map, which contains the address, slot, and new value.
    pub fn writes(&self) -> &HashMap<Address, HashMap<StorageKey, StorageValue>> {
        &self.storage_write
    }

    /// Slots that were only read (SLOAD) but not written (SSTORE)
    pub fn read_only_slots(&self) -> HashMap<Address, HashSet<StorageKey>> {
        self.storage_read
            .iter()
            .map(|(addr, read_slots)| {
                let written = self
                    .storage_write
                    .get(addr)
                    .map(|w| w.keys().cloned().collect::<HashSet<_>>())
                    .unwrap_or_default();

                let read_only = read_slots.difference(&written).cloned().collect();
                (*addr, read_only)
            })
            .collect()
    }

    /// Slots that were written with the same value (no-op SSTORE)
    pub fn unchanged_writes(&self) -> HashMap<Address, HashSet<StorageKey>> {
        self.storage_write
            .iter()
            .map(|(addr, writes)| {
                let binding = Default::default();
                let pre = self.pre_state.get(addr).unwrap_or(&binding);
                let unchanged = writes
                    .iter()
                    .filter(|(slot, new_val)| pre.get(*slot) == Some(*new_val))
                    .map(|(slot, _)| *slot)
                    .collect();
                (*addr, unchanged)
            })
            .collect()
    }

    /// Slots that existed in pre-state but weren't written
    pub fn untouched_pre_state_slots(&self) -> HashMap<Address, HashSet<StorageKey>> {
        self.pre_state
            .iter()
            .map(|(addr, preslots)| {
                let written = self
                    .storage_write
                    .get(addr)
                    .map(|m| m.keys().cloned().collect::<HashSet<_>>())
                    .unwrap_or_default();

                let untouched =
                    preslots.keys().filter(|k| !written.contains(*k)).cloned().collect();

                (*addr, untouched)
            })
            .collect()
    }

    /// Returns all storage slots that were **read** during execution but not considered true
    /// writes.
    ///
    /// Includes:
    /// - Slots accessed via `SLOAD` but not written (`SSTORE`)
    /// - Slots written via `SSTORE` with the same value as the pre-state (no-op writes)
    /// - Slots present in pre-state but untouched (neither read nor written)
    pub fn get_bal_storage_reads(&self) -> HashMap<Address, HashSet<StorageKey>> {
        let mut result = HashMap::<Address, HashSet<StorageKey>>::default();
        for (addr, slots) in self.read_only_slots() {
            result.entry(addr).or_default().extend(slots);
        }
        for (addr, slots) in self.unchanged_writes() {
            result.entry(addr).or_default().extend(slots);
        }
        for (addr, slots) in self.untouched_pre_state_slots() {
            result.entry(addr).or_default().extend(slots);
        }

        result
    }

    /// Returns all storage slots that were written with meaningful changes.
    ///
    /// Includes:
    /// - Value changes: where the post-value differs from the pre-value
    /// - Zeroed slots: where a non-zero pre-value was written to zero
    pub fn get_bal_storage_writes(&self) -> HashMap<Address, HashMap<StorageKey, StorageValue>> {
        let mut writes: HashMap<Address, HashMap<StorageKey, StorageValue>> = HashMap::default();

        for (addr, slots) in &self.storage_write {
            let default_pre = HashMap::default();
            let pre = self.pre_state.get(addr).unwrap_or(&default_pre);

            for (slot, new_val) in slots {
                let old_val = pre.get(slot).copied().unwrap_or_default();
                let changed = new_val != &old_val;
                let zeroed = old_val != StorageValue::ZERO && *new_val == StorageValue::ZERO;

                if changed || zeroed {
                    writes.entry(*addr).or_default().insert(*slot, *new_val);
                }
            }
        }

        writes
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

                    if let Some(revm::JournalEntry::StorageChanged {
                        address: addr,
                        key: slot_key,
                        had_value,
                    }) = journal.journal().last()
                    {
                        if *addr == address && *slot_key == key.into() {
                            let post = journal.evm_state()[addr].storage[slot_key].present_value();
                            self.storage_write
                                .entry(address)
                                .or_default()
                                .entry(key)
                                .or_insert((*had_value, post));
                        }
                    }
                }
            }

            opcode::SSTORE => {
                if let (Ok(val), Ok(slot)) = (interp.stack.peek(0), interp.stack.peek(1)) {
                    let key = StorageKey::from(slot.to_be_bytes());
                    let value = StorageValue::from(val);

                    if let Some(revm::JournalEntry::StorageChanged {
                        address: addr,
                        key: slot_key,
                        had_value,
                    }) = journal.journal().last()
                    {
                        if *addr == address && *slot_key == key.into() {
                            self.storage_write
                                .entry(address)
                                .or_default()
                                .insert(key, (*had_value, value));
                        }
                    }
                }
            }

            _ => {}
        }
    }
}
