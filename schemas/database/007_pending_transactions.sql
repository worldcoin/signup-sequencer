CREATE TABLE transaction_requests (
	id BYTEA PRIMARY KEY,
	received_at INTEGER NOT NULL, -- unix timestamp, seconds
	serialized_tx BYTEA NOT NULL
);

CREATE TABLE submitted_eth_tx (
	transaction_request_id BYTEA NOT NULL REFERENCES transaction_requests(id),
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
