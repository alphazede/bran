CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    region TEXT NOT NULL,
    note TEXT
);
CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    total_cents INTEGER NOT NULL,
    status TEXT NOT NULL
);
CREATE VIEW order_totals AS
    SELECT customer_id, sum(total_cents) AS total_cents
    FROM orders
    GROUP BY customer_id;
INSERT INTO customers VALUES (1, 'Synthetic Customer 001', 'customer001@example.invalid', 'north', NULL);
INSERT INTO customers VALUES (2, 'Synthetic Customer 002', 'customer002@example.invalid', 'south', 'prefers email');
INSERT INTO customers VALUES (3, 'Synthetic Customer 003', 'customer003@example.invalid', 'north', NULL);
INSERT INTO customers VALUES (4, 'Synthetic ''); DROP TABLE orders; --', 'customer004@example.invalid', 'east', 'injection text stored as data');
INSERT INTO customers VALUES (5, 'Synthetic Zoë Ünïcode 005', 'customer005@example.invalid', 'west', 'non-ASCII text');
INSERT INTO customers VALUES (6, 'Synthetic Customer 006', 'customer006@example.invalid', 'south', 'api_key=synthetic-not-a-real-key');
INSERT INTO customers VALUES (7, 'Synthetic Customer 007', 'customer007@example.invalid', 'east', 'long note xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx');
INSERT INTO orders VALUES (1, 1, 1250, 'paid');
INSERT INTO orders VALUES (2, 1, 800, 'shipped');
INSERT INTO orders VALUES (3, 2, 4500, 'paid');
INSERT INTO orders VALUES (4, 3, 99, 'void');
INSERT INTO orders VALUES (5, 4, 3000, 'paid');
INSERT INTO orders VALUES (6, 5, 1725, 'shipped');
INSERT INTO orders VALUES (7, 6, 640, 'paid');
INSERT INTO orders VALUES (8, 7, 2210, 'pending');
