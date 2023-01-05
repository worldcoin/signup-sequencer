-- note: BYTEA does not exist in sqlite, so it must use a different migration

CREATE TABLE tx_requests (
	id BYTEA PRIMARY KEY,
	received_at INTEGER NOT NULL, -- unix timestamp, seconds
	serialized_tx BYTEA NOT NULL
);

CREATE TABLE submitted_eth_tx (
	tx_request_id BYTEA NOT NULL REFERENCES tx_requests(id),
	submitted_at INTEGER NOT NULL,

	tx_hash BYTEA NOT NULL CHECK(length(tx_hash) = 32),
	serialized_tx BYTEA NOT NULL,  -- the rlp-encoded transaction, including signature,
	                              -- ready for resubmission

	-- these are reciept fields, null if not yet mined
	block_number INTEGER,
	block_hash BYTEA CHECK(length(block_hash) = 32),

	-- these are big-endian 256-bit integers
	gas_used BYTEA CHECK(length(gas_used) = 32),
	effective_gas_price BYTEA CHECK(length(effective_gas_price) = 32)
);
