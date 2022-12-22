CREATE TABLE tx_requests (
	id BLOB PRIMARY KEY,
	received_at INTEGER NOT NULL, -- unix timestamp, seconds
	serialized_tx BLOB NOT NULL,
)

CREATE TABLE submitted_eth_tx (
	tx_request_id BLOB NOT NULL REFERENCES tx_requests(id),
	submitted_at INTEGER NOT NULL,

	tx_hash BLOB NOT NULL CHECK(length(tx_hash) == 32),
	serialized_tx BLOB NOT NULL,  -- the rlp-encoded transaction, including signature,
	                              -- ready for resubmission

	-- these are reciept fields, null if not yet mined
	block_number INTEGER,
	block_hash BLOB CHECK(length(block_hash) == 32),

	-- these are big-endian 256-bit integers
	gas_used BLOB CHECK(length(gas_used) == 32),
	effective_gas_price BLOB CHECK(length(effective_gas_price) == 32),
)
