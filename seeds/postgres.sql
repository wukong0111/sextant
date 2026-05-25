-- PostgreSQL seed data for sextant Docker test DB
DROP TABLE IF EXISTS type_samples CASCADE;
DROP TABLE IF EXISTS products CASCADE;
DROP TABLE IF EXISTS orders CASCADE;
DROP TABLE IF EXISTS users CASCADE;

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    email TEXT NOT NULL UNIQUE,
    active BOOLEAN DEFAULT true,
    age SMALLINT,
    score REAL,
    balance NUMERIC(12,4),
    birth_date DATE,
    last_login TIMESTAMPTZ,
    profile JSONB,
    tags TEXT[],
    avatar BYTEA,
    uuid UUID DEFAULT gen_random_uuid()
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INT NOT NULL REFERENCES users(id),
    total NUMERIC(10,2) NOT NULL,
    status VARCHAR(20) DEFAULT 'pending',
    metadata JSON,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    price NUMERIC(10,2) NOT NULL,
    stock INTEGER DEFAULT 0,
    weight DOUBLE PRECISION,
    is_available BOOLEAN DEFAULT true,
    specs JSONB,
    image BYTEA,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE type_samples (
    id SERIAL PRIMARY KEY,
    c_bool BOOLEAN,
    c_smallint SMALLINT,
    c_int INTEGER,
    c_bigint BIGINT,
    c_real REAL,
    c_double DOUBLE PRECISION,
    c_numeric NUMERIC(15,5),
    c_varchar VARCHAR(100),
    c_text TEXT,
    c_char CHAR(5),
    c_date DATE,
    c_time TIME,
    c_timestamp TIMESTAMP,
    c_timestamptz TIMESTAMPTZ,
    c_json JSON,
    c_jsonb JSONB,
    c_bytea BYTEA,
    c_text_array TEXT[],
    c_int_array INT[],
    c_uuid UUID,
    c_inet INET,
    c_varchar_unicode VARCHAR(100)
);

INSERT INTO users (name, email, active, age, score, balance, birth_date, last_login, profile, tags, avatar, uuid) VALUES
    ('Alice', 'alice@example.com', true, 30, 95.5, 1234.5678, '1994-05-12', '2024-06-15 14:30:00+00', '{"city": "NYC", "premium": true}', ARRAY['admin', 'beta'], '\xDEADBEEF', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'),
    ('Bob', 'bob@example.com', false, 25, 82.0, -50.0000, '1999-11-03', NULL, '{}', ARRAY['user'], NULL, 'b0eebc99-9c0b-4ef8-bb6d-6bb9bd380a12'),
    ('Carol', 'carol@example.com', true, 42, 88.25, 999999.9999, '1982-01-20', '2023-12-25 08:00:00+00', '{"city": "Berlin", "premium": false, "scores": [10, 20, 30]}', ARRAY[]::text[], '\xCAFE00', 'c0eebc99-9c0b-4ef8-bb6d-6bb9bd380a13'),
    ('Dani 🚀', 'dani@example.com', true, NULL, NULL, 0.0000, '2000-01-01', '2025-01-01 00:00:00+00', '{"emoji": "🚀", "unicode": "日本語"}', ARRAY['user', 'admin', 'qa'], '\x00FF00', 'd0eebc99-9c0b-4ef8-bb6d-6bb9bd380a14');

INSERT INTO orders (user_id, total, status, metadata, created_at) VALUES
    (1, 99.99, 'completed', '{"coupon": "SUMMER24"}', '2024-01-10 10:00:00'),
    (1, 45.50, 'pending', '{}', '2024-02-20 16:45:00'),
    (2, 120.00, 'completed', NULL, '2023-11-11 11:11:11'),
    (3, 0.01, 'cancelled', '{"reason": "test order", "items": [{"sku": "A1", "qty": 1}]}', '2024-03-03 03:03:03'),
    (4, 9999.99, 'pending', '{"bulk": true}', '2025-06-15 12:00:00');

INSERT INTO products (name, description, price, stock, weight, is_available, specs, image, created_at) VALUES
    ('Ergonomic Keyboard', 'A nice mechanical keyboard', 149.99, 42, 1.2, true, '{"layout": "ISO", "switches": "brown"}', '\x89504E47', '2024-01-01 00:00:00+00'),
    ('Wireless Mouse', NULL, 29.99, 0, 0.08, false, '{}', NULL, '2024-06-01 12:30:00+00'),
    ('USB-C Cable', '1m braided cable', 9.99, 500, 0.05, true, '{"length": 1, "color": "black"}', '\xFFD8FF', '2023-09-15 08:00:00+00');

INSERT INTO type_samples (
    c_bool, c_smallint, c_int, c_bigint, c_real, c_double, c_numeric,
    c_varchar, c_text, c_char, c_date, c_time, c_timestamp, c_timestamptz,
    c_json, c_jsonb, c_bytea, c_text_array, c_int_array, c_uuid, c_inet, c_varchar_unicode
) VALUES
    (true, -32768, -2147483648, -9223372036854775808, 3.14, 2.718281828459045, 12345.67890,
     'hello', 'long text with\nnewlines and    spaces', 'ABCDE', '2024-01-01', '12:34:56', '2024-06-15 14:30:00', '2024-06-15 14:30:00+00',
     '{"a": 1, "b": [true, false]}', '{"nested": {"key": "value"}}', '\x00010203',
     ARRAY['one', 'two', 'three'], ARRAY[1, 2, 3], 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '192.168.1.1', '日本語 🎉'),
    (false, 32767, 2147483647, 9223372036854775807, -1.5e38, 1.7e308, -0.00001,
     '', '', '     ', '1999-12-31', '00:00:00', '1970-01-01 00:00:00', '2000-01-01 00:00:00+00',
     '[]', '{}', '\x',
     ARRAY[]::text[], ARRAY[]::int[], '00000000-0000-0000-0000-000000000000', '0.0.0.0', ''),
    (NULL, NULL, NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL,
     NULL, NULL, NULL, NULL, NULL);
