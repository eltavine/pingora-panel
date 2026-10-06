-- resty.websocket.client: WebSockets (RFC 6455) a script opens over a
-- cosocket, as lua-resty-websocket gives them.
local protocol = require "resty.websocket.protocol"
local random = require "resty.random"
local band, rshift = bit32.band, bit32.rshift
local char, find, lower, match = string.char, string.find, string.lower, string.match
local concat = table.concat

local client = { version = protocol.version }
local methods = { __index = client }

-- RFC 6455 §1.3.
local GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

function client.new(self, opts)
    opts = opts or {}
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    if opts.timeout then
        sock:settimeout(opts.timeout)
    end
    local most = opts.max_payload_len or 65536
    return setmetatable({
        sock = sock,
        max_recv_len = opts.max_recv_len or most,
        max_send_len = opts.max_send_len or most,
        send_unmasked = opts.send_unmasked,
    }, methods)
end

-- The scheme, host, port and path of a ws:// or wss:// URI.
local function parse(uri)
    local scheme, rest = match(uri, "^(wss?)://(.+)$")
    if not scheme then
        return nil
    end
    local authority, path = match(rest, "^([^/?#]+)(.*)$")
    if not authority then
        return nil
    end
    local host, port = match(authority, "^%[([^%]]+)%]:?(%d*)$")
    if not host then
        host, port = match(authority, "^([^:]+):?(%d*)$")
    end
    if not host then
        return nil
    end
    port = tonumber(port) or (scheme == "wss" and 443 or 80)
    if path == "" or find(path, "^[?#]") then
        path = "/" .. path
    end
    return scheme, host, port, (match(path, "^([^#]*)"))
end

-- The value of the response header `name` in the raw `head`, ignoring case.
local function header(head, name)
    for line in string.gmatch(head, "[^\r\n]+") do
        local field, value = match(line, "^([^:]+):%s*(.-)%s*$")
        if field and lower(field) == name then
            return value
        end
    end
    return nil
end

function client.connect(self, uri, opts)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local scheme, host, port, path = parse(uri)
    if not scheme then
        return nil, "bad websocket uri"
    end
    opts = opts or {}
    local ok, err = sock:connect(host, port, {
        pool = opts.pool or (host .. ":" .. port),
        pool_size = opts.pool_size,
        backlog = opts.backlog,
    })
    if not ok then
        return nil, "failed to connect: " .. err
    end
    self.closed = nil
    self.fatal = nil
    if sock:getreusedtimes() > 0 then
        return 1, nil, "connection reused"
    end
    if scheme == "wss" then
        if opts.client_cert then
            ok, err = sock:setclientcert(opts.client_cert, opts.client_priv_key)
            if not ok then
                return nil, "failed to set TLS client certificate: " .. err
            end
        end
        ok, err = sock:sslhandshake(false, opts.server_name or opts.host or host, opts.ssl_verify)
        if not ok then
            return nil, "ssl handshake failed: " .. err
        end
    end
    local key = opts.key or ngx.encode_base64(random.bytes(16))
    local default_port = (scheme == "wss" and port == 443) or (scheme == "ws" and port == 80)
    local authority = find(host, ":", 1, true) and ("[" .. host .. "]") or host
    local lines = {
        "GET " .. path .. " HTTP/1.1",
        "Upgrade: websocket",
        "Host: " .. (opts.host or (default_port and authority or (authority .. ":" .. port))),
        "Sec-WebSocket-Key: " .. key,
        "Sec-WebSocket-Version: 13",
        "Connection: Upgrade",
    }
    local protocols = opts.protocols
    if type(protocols) == "table" then
        protocols = concat(protocols, ",")
    end
    if protocols then
        lines[#lines + 1] = "Sec-WebSocket-Protocol: " .. protocols
    end
    if opts.origin then
        lines[#lines + 1] = "Origin: " .. opts.origin
    end
    if opts.headers then
        for _, line in ipairs(opts.headers) do
            lines[#lines + 1] = line
        end
    end
    local bytes
    bytes, err = sock:send(concat(lines, "\r\n") .. "\r\n\r\n")
    if not bytes then
        return nil, "failed to send the handshake request: " .. err
    end
    local head
    head, err = sock:receiveuntil("\r\n\r\n")()
    if not head then
        return nil, "failed to receive response header: " .. err
    end
    local status = match(head, "^HTTP/1%.1 (%d%d%d)")
    if not status then
        return nil, "bad HTTP response status line: " .. head, head
    end
    if status ~= "101" then
        return nil, "failed websocket handshake: unexpected response status: " .. status, head
    end
    local accept = header(head, "sec-websocket-accept")
    if accept ~= ngx.encode_base64(ngx.sha1_bin(key .. GUID)) then
        return nil, "failed websocket handshake: bad \"sec-websocket-accept\" response header", head
    end
    return 1, nil, head
end

function client.set_timeout(self, time)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:settimeout(time)
end

function client.recv_frame(self)
    if self.fatal then
        return nil, nil, "fatal error already happened"
    end
    local sock = self.sock
    if not sock then
        return nil, nil, "not initialized"
    end
    local data, typ, err = protocol.recv_frame(sock, self.max_recv_len, false)
    if not data and not find(err, ": timeout", 1, true) then
        self.fatal = true
    end
    return data, typ, err
end

local function send_frame(self, fin, opcode, payload)
    if self.fatal then
        return nil, "fatal error already happened"
    end
    if self.closed then
        return nil, "already closed"
    end
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local bytes, err = protocol.send_frame(sock, fin, opcode, payload, self.max_send_len, not self.send_unmasked)
    if not bytes then
        self.fatal = true
    end
    return bytes, err
end

client.send_frame = send_frame

function client.send_text(self, data)
    return send_frame(self, true, 0x1, data)
end

function client.send_binary(self, data)
    return send_frame(self, true, 0x2, data)
end

function client.send_close(self, code, msg)
    local payload
    if code then
        if type(code) ~= "number" or code < 0 or code > 0x7fff then
            return nil, "bad status code"
        end
        payload = char(band(rshift(code, 8), 0xff), band(code, 0xff)) .. (msg or "")
    end
    local bytes, err = send_frame(self, true, 0x8, payload)
    if bytes then
        self.closed = true
    end
    return bytes, err
end

function client.send_ping(self, data)
    return send_frame(self, true, 0x9, data)
end

function client.send_pong(self, data)
    return send_frame(self, true, 0xa, data)
end

function client.close(self)
    if self.fatal then
        return nil, "fatal error already happened"
    end
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if not self.closed then
        local bytes, err = send_frame(self, true, 0x8, "")
        if not bytes then
            return nil, "failed to send close frame: " .. err
        end
        self.closed = true
    end
    return sock:close()
end

function client.set_keepalive(self, max_idle_timeout, pool_size)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if self.fatal then
        return nil, "failed to set keepalive: fatal error already happened"
    end
    self.closed = true
    return sock:setkeepalive(max_idle_timeout, pool_size)
end

return client
