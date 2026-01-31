-- Schema for Monk (Monastery) System

-- Tenants table: represents tenants in the monastery
CREATE TABLE tenants (
    tenant_id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    contact_info VARCHAR(255),
    room_number VARCHAR(50),
    move_in_date DATE,
    move_out_date DATE
);

-- Requests table: represents requests made by tenants
CREATE TABLE requests (
    request_id SERIAL PRIMARY KEY,
    tenant_id INTEGER NOT NULL REFERENCES tenants(tenant_id) ON DELETE CASCADE,
    request_type VARCHAR(100) NOT NULL,
    description TEXT,
    status VARCHAR(50) DEFAULT 'pending',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved_at TIMESTAMP
);

-- Example insert statements
INSERT INTO tenants (name, contact_info, room_number, move_in_date) VALUES
('Brother John', 'john@example.com', 'A101', '2023-01-15'),
('Sister Mary', 'mary@example.com', 'B202', '2023-02-01');

INSERT INTO requests (tenant_id, request_type, description) VALUES
(1, 'Maintenance', 'Leaky faucet in room A101'),
(2, 'Food', 'Request for vegetarian meals');

-- Example query to join tenants and their requests
SELECT t.name, t.room_number, r.request_type, r.status, r.created_at
FROM tenants t
LEFT JOIN requests r ON t.tenant_id = r.tenant_id
ORDER BY t.name, r.created_at;
