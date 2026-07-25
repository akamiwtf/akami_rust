# akami_rust

Rust-порт бэкенда Akami (Discord-клон). Полностью заменяет Node-сервер из
`../akamiwtf/server`: тот же REST API, те же Socket.io-события, тот же формат
шифрования DM и совместимость с существующей базой `dev.db` и JWT-токенами.

## Стек

axum · socketioxide (Socket.io) · sqlx (SQLite) · jsonwebtoken · bcrypt · AES-256-CBC

## Конфигурация (`.env`)

```
PORT=5000
JWT_SECRET=<длинная случайная строка>
DM_ENCRYPTION_KEY=<длинная случайная строка; по умолчанию берётся JWT_SECRET>
DATABASE_URL=sqlite:dev.db
```

Оба секрета должны совпадать со значениями Node-версии, иначе выданные ранее
JWT перестанут проходить проверку, а существующие DM не расшифруются.
`DATABASE_URL` можно указать на существующую базу — схема создаётся
идемпотентно и старые данные не трогаются.

## Запуск

```powershell
# из папки проекта
cargo run --release
```

Сервер поднимается на `PORT` (по умолчанию 5000): REST на `/api/*`, Socket.io на
`/socket.io/`, загрузки на `/uploads/`.

## Клиент

Это **чистый API-сервер** — статику он не раздаёт. Единственный клиент —
десктоп-приложение `../akami_desktop` (Electron), которое обращается сюда по
`http://localhost:5000` (см. `akami_desktop/client/.env`).

```powershell
cd ..\akami_desktop\client
npm run dev        # http://localhost:5173
npm run electron   # окно приложения
```

## Тесты

```powershell
cargo test
```

Покрывают: совместимость с реальной `dev.db`, JWT/bcrypt/AES против эталонов
Node, полный REST-цикл по HTTP и Socket.io-обмен (сообщения, редактирование,
DM с шифрованием, голос) настоящим Rust-клиентом socket.io.
