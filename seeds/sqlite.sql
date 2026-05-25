-- SQLite seed data for sextant test DB
PRAGMA foreign_keys = ON;

DROP TABLE IF EXISTS type_samples;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS users;

CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    active INTEGER DEFAULT 1,
    age INTEGER,
    score REAL,
    balance REAL,
    birth_date TEXT,
    last_login TEXT,
    profile TEXT,
    tags TEXT,
    avatar BLOB,
    uuid TEXT
);

CREATE TABLE orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    total REAL NOT NULL,
    status TEXT DEFAULT 'pending',
    metadata TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    price REAL NOT NULL,
    stock INTEGER DEFAULT 0,
    weight REAL,
    is_available INTEGER DEFAULT 1,
    specs TEXT,
    image BLOB,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE type_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    c_integer INTEGER,
    c_real REAL,
    c_text TEXT,
    c_blob BLOB,
    c_numeric NUMERIC,
    c_varchar TEXT,
    c_date TEXT,
    c_datetime TEXT,
    c_json TEXT,
    c_boolean INTEGER,
    c_unicode TEXT
);

INSERT INTO users (name, email, active, age, score, balance, birth_date, last_login, profile, tags, avatar, uuid) VALUES
    ('Alice', 'alice@example.com', 1, 30, 95.5, 1234.5678, '1994-05-12', '2024-06-15 14:30:00', '{"city": "NYC", "premium": true}', '["admin", "beta"]', X'DEADBEEF', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'),
    ('Bob', 'bob@example.com', 0, 25, 82.0, -50.0000, '1999-11-03', NULL, '{}', '["user"]', NULL, 'b0eebc99-9c0b-4ef8-bb6d-6bb9bd380a12'),
    ('Carol', 'carol@example.com', 1, 42, 88.25, 999999.9999, '1982-01-20', '2023-12-25 08:00:00', '{"city": "Berlin", "premium": false, "scores": [10, 20, 30]}', '[]', X'CAFE00', 'c0eebc99-9c0b-4ef8-bb6d-6bb9bd380a13'),
    ('Dani 🚀', 'dani@example.com', 1, NULL, NULL, 0.0000, '2000-01-01', '2025-01-01 00:00:00', '{"emoji": "🚀", "unicode": "日本語"}', '["user", "admin", "qa"]', X'00FF00', 'd0eebc99-9c0b-4ef8-bb6d-6bb9bd380a14');

INSERT INTO orders (user_id, total, status, metadata, created_at) VALUES
    (1, 99.99, 'completed', '{"coupon": "SUMMER24"}', '2024-01-10 10:00:00'),
    (1, 45.50, 'pending', '{}', '2024-02-20 16:45:00'),
    (2, 120.00, 'completed', NULL, '2023-11-11 11:11:11'),
    (3, 0.01, 'cancelled', '{"reason": "test order", "items": [{"sku": "A1", "qty": 1}]}', '2024-03-03 03:03:03'),
    (4, 9999.99, 'pending', '{"bulk": true}', '2025-06-15 12:00:00');

INSERT INTO products (name, description, price, stock, weight, is_available, specs, image, created_at) VALUES
    ('Ergonomic Keyboard', 'A nice mechanical keyboard', 149.99, 42, 1.2, 1, '{"layout": "ISO", "switches": "brown"}', X'89504E47', '2024-01-01 00:00:00'),
    ('Wireless Mouse', NULL, 29.99, 0, 0.08, 0, '{}', NULL, '2024-06-01 12:30:00'),
    ('USB-C Cable', '1m braided cable', 9.99, 500, 0.05, 1, '{"length": 1, "color": "black"}', X'FFD8FF', '2023-09-15 08:00:00');

INSERT INTO type_samples (
    c_integer, c_real, c_text, c_blob, c_numeric,
    c_varchar, c_date, c_datetime, c_json, c_boolean, c_unicode
) VALUES
    (-2147483648, 3.14, 'hello', X'00010203', 12345.67890,
     'varchar value', '2024-01-01', '2024-06-15 14:30:00', '{"a": 1, "b": [true, false]}', 1, '日本語 🎉'),
    (2147483647, -1.79769e308, 'long text with\nnewlines and    spaces', X'', -0.00001,
     '', '1999-12-31', '1970-01-01 00:00:00', '[]', 0, ''),
    (NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL, NULL, NULL, NULL);
