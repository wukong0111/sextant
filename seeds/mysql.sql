-- MySQL seed data for sextant Docker test DB
DROP TABLE IF EXISTS type_samples;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS users;

CREATE TABLE users (
    id INT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    email TEXT NOT NULL,
    active TINYINT(1) DEFAULT 1,
    age SMALLINT,
    score FLOAT,
    balance DECIMAL(12,4),
    birth_date DATE,
    last_login DATETIME,
    profile JSON,
    tags JSON,
    avatar BLOB,
    uuid CHAR(36)
);

CREATE TABLE orders (
    id INT AUTO_INCREMENT PRIMARY KEY,
    user_id INT NOT NULL,
    total DECIMAL(10,2) NOT NULL,
    status ENUM('pending', 'completed', 'cancelled') DEFAULT 'pending',
    metadata JSON,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE TABLE products (
    id INT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    price DECIMAL(10,2) NOT NULL,
    stock INT DEFAULT 0,
    weight DOUBLE,
    is_available TINYINT(1) DEFAULT 1,
    specs JSON,
    image BLOB,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE type_samples (
    id INT AUTO_INCREMENT PRIMARY KEY,
    c_bool TINYINT(1),
    c_smallint SMALLINT,
    c_int INT,
    c_bigint BIGINT,
    c_float FLOAT,
    c_double DOUBLE,
    c_decimal DECIMAL(15,5),
    c_varchar VARCHAR(100),
    c_text TEXT,
    c_char CHAR(5),
    c_date DATE,
    c_time TIME,
    c_datetime DATETIME,
    c_timestamp TIMESTAMP,
    c_json JSON,
    c_blob BLOB,
    c_binary BINARY(16),
    c_enum ENUM('a', 'b', 'c'),
    c_varchar_unicode VARCHAR(100)
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
    c_bool, c_smallint, c_int, c_bigint, c_float, c_double, c_decimal,
    c_varchar, c_text, c_char, c_date, c_time, c_datetime, c_timestamp,
    c_json, c_blob, c_binary, c_enum, c_varchar_unicode
) VALUES
    (1, -32768, -2147483648, -9223372036854775808, 3.14, 2.718281828459045, 12345.67890,
     'hello', 'long text with\nnewlines and    spaces', 'ABCDE', '2024-01-01', '12:34:56', '2024-06-15 14:30:00', '2024-06-15 14:30:00',
     '{"a": 1, "b": [true, false]}', X'00010203', X'000102030405060708090A0B0C0D0E0F', 'a', '日本語 🎉'),
    (0, 32767, 2147483647, 9223372036854775807, -1.5e38, 1.7e308, -0.00001,
     '', '', '     ', '1999-12-31', '00:00:00', '1970-01-01 00:00:00', '2000-01-01 00:00:00',
     '[]', X'', X'00000000000000000000000000000000', 'b', ''),
    (NULL, NULL, NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL, NULL, NULL);
