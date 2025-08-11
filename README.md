# MinePing

REST API for querying Minecraft server status. Supports modern and legacy Server List Ping protocols, plus Query protocol.

## Running the Server

```bash
# Default port 3000
./mineping

# Custom port
PORT=8080 ./mineping
```

## API Usage

The API uses a ticket-based system. Request a ping to get a ticket, then check the ticket status.

### Request a server ping

```
GET /ping/{host}
GET /ping/{host}/{port}
```

Optional query parameter:
- `fresh=true` - Bypass cache

Returns:
```json
{
  "ticket_id": "abc123...",
  "check_url": "/status/abc123..."
}
```

### Check ticket status

```
GET /status/{ticket_id}
```

Returns one of:

**Pending:**
```json
{
  "ticket_id": "abc123...",
  "status": "pending",
  "position": 3,
  "created_at": 1234567890,
  "age_seconds": 2
}
```

**Processing:**
```json
{
  "ticket_id": "abc123...",
  "status": "processing",
  "created_at": 1234567890,
  "age_seconds": 5
}
```

**Ready:**
```json
{
  "ticket_id": "abc123...",
  "status": "ready",
  "result": {
    "online": true,
    "host": "mc.example.com",
    "port": 25565,
    "resolved_from": "srv",
    "version": {
      "name": "1.20.4",
      "protocol": 765
    },
    "players": {
      "online": 5,
      "max": 20,
      "sample": ["Steve", "Alex"]
    },
    "description": "A Minecraft Server",
    "favicon": "data:image/png;base64,...",
    "latency_ms": 42,
    "query_enabled": false,
    "protocol_used": "modern_slp"
  },
  "created_at": 1234567890,
  "age_seconds": 7
}
```

**Error:**
```json
{
  "ticket_id": "abc123...",
  "status": "error",
  "message": "Request timed out",
  "created_at": 1234567890,
  "age_seconds": 10
}
```

### Other Endpoints

```
GET /health    - Service health and queue status
GET /stats     - Request statistics and protocol usage
```

## Example Usage

```bash
# Request a ping
curl http://localhost:3000/ping/mc.hypixel.net
# {"ticket_id":"abc123...","check_url":"/status/abc123..."}

# Check status
curl http://localhost:3000/status/abc123...
# {"ticket_id":"abc123...","status":"ready","result":{...}}
```

## Notes

- Tickets expire after 5 minutes
- Results are cached for 1 minute
- Supports SRV record resolution
- 10 concurrent workers process ping requests
- 5 second timeout per ping attempt