-- resty.websocket.server: a request's WebSocket (RFC 6455) once the
-- handler answers its opening handshake, as lua-resty-websocket gives it.
local protocol = require "resty.websocket.protocol"
local band, rshift = bit32.band, bit32.rshift
local char, find, lower = string.char, string.find, string.lower

local server = { version = protocol.version }
local methods = { __index = server }

-- RFC 6455 §1.3.
local GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

local function single(value)
    if type(value) == "table" then
        return value[1]
    end
    return value
end

local function joined(value)
    if type(value) == "table" then
        return table.concat(value, ",")
    end
    return value
end

-- Whether the comma-separated `list` holds `token`, ignoring case.
local function lists(list, token)
    for item in string.gmatch(lower(list), "[^,]+") do
        if string.match(item, "^%s*(.-)%s*$") == token then
            return true
        end
    end
    return false
end

-- The subprotocol to answer with: the first the client offers that
-- `allowed` names, or its first when nothing is named.
local function subprotocol(offered, allowed)
    if not offered then
        return nil
    end
    if type(allowed) == "string" then
        allowed = { allowed }
    end
    for name in string.gmatch(offered, "[^,%s]+") do
        if not allowed then
            return name
        end
        for _, wanted in ipairs(allowed) do
            if wanted == name then
                return name
            end
        end
    end
    return nil
end

function server.new(self, opts)
    opts = opts or {}
    if ngx.headers_sent then
        return nil, "response header already sent"
    end
    if ngx.req.http_version() ~= 1.1 then
        return nil, "bad http version"
    end
    local headers = ngx.req.get_headers()
    local upgrade = single(headers.upgrade)
    if not upgrade or lower(upgrade) ~= "websocket" then
        return nil, "bad \"upgrade\" request header: " .. tostring(upgrade)
    end
    local connection = joined(headers.connection)
    if not connection or not lists(connection, "upgrade") then
        return nil, "bad \"connection\" request header"
    end
    local key = single(headers["sec-websocket-key"])
    if not key then
        return nil, "bad \"sec-websocket-key\" request header"
    end
    local version = single(headers["sec-websocket-version"])
    if version ~= "13" then
        return nil, "bad \"sec-websocket-version\" request header"
    end
    local chosen = subprotocol(joined(headers["sec-websocket-protocol"]), opts.protocols)
    ngx.header["Upgrade"] = "websocket"
    ngx.header["Connection"] = "Upgrade"
    ngx.header["Sec-WebSocket-Accept"] = ngx.encode_base64(ngx.sha1_bin(key .. GUID))
    if chosen then
        ngx.header["Sec-WebSocket-Protocol"] = chosen
    end
    ngx.header["Content-Type"] = nil
    ngx.status = 101
    local ok, err = ngx.send_headers()
    if not ok then
        return nil, "failed to send response header: " .. (err or "unknown")
    end
    ok, err = ngx.flush(true)
    if not ok then
        return nil, "failed to flush response header: " .. (err or "unknown")
    end
    local sock
    sock, err = ngx.req.socket(true)
    if not sock then
        return nil, err
    end
    if opts.timeout then
        sock:settimeout(opts.timeout)
    end
    local most = opts.max_payload_len or 65535
    return setmetatable({
        sock = sock,
        max_recv_len = opts.max_recv_len or most,
        max_send_len = opts.max_send_len or most,
        send_masked = opts.send_masked,
    }, methods)
end

function server.set_timeout(self, time)
    local sock = self.sock
    if not sock then
        return nil, "not initialized yet"
    end
    return sock:settimeout(time)
end

function server.recv_frame(self)
    if self.fatal then
        return nil, nil, "fatal error already happened"
    end
    local sock = self.sock
    if not sock then
        return nil, nil, "not initialized yet"
    end
    local data, typ, err = protocol.recv_frame(sock, self.max_recv_len, true)
    if not data and not find(err, ": timeout", 1, true) then
        self.fatal = true
    end
    return data, typ, err
end

local function send_frame(self, fin, opcode, payload)
    if self.fatal then
        return nil, "fatal error already happened"
    end
    local sock = self.sock
    if not sock then
        return nil, "not initialized yet"
    end
    local bytes, err = protocol.send_frame(sock, fin, opcode, payload, self.max_send_len, self.send_masked)
    if not bytes then
        self.fatal = true
    end
    return bytes, err
end

server.send_frame = send_frame

function server.send_text(self, data)
    return send_frame(self, true, 0x1, data)
end

function server.send_binary(self, data)
    return send_frame(self, true, 0x2, data)
end

function server.send_close(self, code, msg)
    local payload
    if code then
        if type(code) ~= "number" or code < 0 or code > 0x7fff then
            return nil, "bad status code"
        end
        payload = char(band(rshift(code, 8), 0xff), band(code, 0xff)) .. (msg or "")
    end
    return send_frame(self, true, 0x8, payload)
end

function server.send_ping(self, data)
    return send_frame(self, true, 0x9, data)
end

function server.send_pong(self, data)
    return send_frame(self, true, 0xa, data)
end

return server
