-- resty.mysql: lua-resty-mysql's client of the MySQL and MariaDB text
-- protocol on cosockets: the v10 handshake with TLS, the native password,
-- caching_sha2_password and sha256_password plugins, result sets, OK and
-- ERR packets, and multiple results.
local byte, char, sub, find = string.byte, string.char, string.sub, string.find
local pack, unpack = string.pack, string.unpack
local concat, create = table.concat, table.create
local bxor = bit32.bxor
local sha256 = require "resty.sha256"

local mysql = { _VERSION = "0.27" }
local methods = { __index = mysql }

-- Capability flags of the client/server protocol.
local LONG_PASSWORD = 0x1
local LONG_FLAG = 0x4
local CONNECT_WITH_DB = 0x8
local PROTOCOL_41 = 0x200
local SSL = 0x800
local TRANSACTIONS = 0x2000
local SECURE_CONNECTION = 0x8000
local MULTI_STATEMENTS = 0x10000
local MULTI_RESULTS = 0x20000
local PS_MULTI_RESULTS = 0x40000
local PLUGIN_AUTH = 0x80000

local CLIENT_FLAGS = LONG_PASSWORD + LONG_FLAG + PROTOCOL_41 + TRANSACTIONS + SECURE_CONNECTION
    + MULTI_STATEMENTS + MULTI_RESULTS + PS_MULTI_RESULTS + PLUGIN_AUTH

local SERVER_MORE_RESULTS_EXISTS = 0x8

local COM_QUERY = "\3"
local COM_QUIT = "\1"

-- The default collation of each character set, by MySQL's numbering.
local CHARSETS = {
    big5 = 1, dec8 = 3, cp850 = 4, hp8 = 6, koi8r = 7, latin1 = 8, latin2 = 9, swe7 = 10,
    ascii = 11, ujis = 12, sjis = 13, hebrew = 16, tis620 = 18, euckr = 19, koi8u = 22,
    gb2312 = 24, greek = 25, cp1250 = 26, gbk = 28, latin5 = 30, armscii8 = 32, utf8 = 33,
    ucs2 = 35, cp866 = 36, keybcs2 = 37, macce = 38, macroman = 39, cp852 = 40, latin7 = 41,
    utf8mb4 = 45, cp1251 = 51, utf16 = 54, utf16le = 56, cp1256 = 57, cp1257 = 59, utf32 = 60,
    binary = 63, geostd8 = 92, cp932 = 95, eucjpms = 97, gb18030 = 248,
}

-- Column types whose text values read as numbers.
local NUMERIC = {
    [0x00] = true, [0x01] = true, [0x02] = true, [0x03] = true, [0x04] = true, [0x05] = true,
    [0x08] = true, [0x09] = true, [0x0d] = true, [0xf6] = true,
}

local READY, SENT, PENDING = "ready", "sent", "pending"

function mysql.new(self)
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    return setmetatable({ sock = sock }, methods)
end

function mysql.set_timeout(self, timeout)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:settimeout(timeout)
end

function mysql.set_compact_arrays(self, value)
    self.compact = value
end

function mysql.server_ver(self)
    return self._server_ver
end

local function send_packet(self, payload)
    self.packet_no = (self.packet_no + 1) % 256
    return self.sock:send(pack("<I3B", #payload, self.packet_no) .. payload)
end

-- One packet's payload, with those continuing a 16 MiB one appended.
local function read_packet(self)
    local sock = self.sock
    local parts
    while true do
        local head, err = sock:receive(4)
        if not head then
            return nil, "failed to receive packet header: " .. err
        end
        local length, number = unpack("<I3B", head)
        if length > self.max_packet_size then
            return nil, "packet size too big: " .. length
        end
        self.packet_no = number
        local payload = ""
        if length > 0 then
            payload, err = sock:receive(length)
            if not payload then
                return nil, "failed to read packet content: " .. err
            end
        end
        if length < 0xffffff and not parts then
            return payload
        end
        parts = parts or {}
        parts[#parts + 1] = payload
        if length < 0xffffff then
            return concat(parts)
        end
    end
end

-- A length-encoded integer at `at`, the position after it, and whether it
-- was NULL's 0xfb.
local function lenenc(data, at)
    local first = byte(data, at)
    if first == nil then
        return nil, at
    end
    if first < 0xfb then
        return first, at + 1
    elseif first == 0xfb then
        return nil, at + 1, true
    elseif first == 0xfc then
        return unpack("<I2", data, at + 1), at + 3
    elseif first == 0xfd then
        return unpack("<I3", data, at + 1), at + 4
    end
    return unpack("<I8", data, at + 1), at + 9
end

local function lenenc_string(data, at)
    local length, next_at, null = lenenc(data, at)
    if null or not length then
        return nil, next_at
    end
    return sub(data, next_at, next_at + length - 1), next_at + length
end

local function error_of(payload)
    local code = unpack("<I2", payload, 2)
    local state, message
    if sub(payload, 4, 4) == "#" then
        state, message = sub(payload, 5, 9), sub(payload, 10)
    else
        message = sub(payload, 4)
    end
    return message, code, state
end

local function ok_of(payload)
    local affected, insert_id, at
    affected, at = lenenc(payload, 2)
    insert_id, at = lenenc(payload, at)
    local status, warnings = unpack("<I2I2", payload, at)
    local message = sub(payload, at + 4)
    return {
        affected_rows = affected,
        insert_id = insert_id,
        server_status = status,
        warning_count = warnings,
        message = message ~= "" and message or nil,
    }, status
end

local function xor(left, right)
    local out = create(#left)
    for index = 1, #left do
        out[index] = char(bxor(byte(left, index), byte(right, index)))
    end
    return concat(out)
end

local function sha256_of(data)
    local hasher = sha256:new()
    hasher:update(data)
    return hasher:final()
end

-- What a plugin answers the server's `scramble` with.
local function auth_response(plugin, password, scramble)
    if password == "" then
        return ""
    end
    if plugin == "mysql_native_password" then
        local stage1 = ngx.sha1_bin(password)
        return xor(stage1, ngx.sha1_bin(sub(scramble, 1, 20) .. ngx.sha1_bin(stage1)))
    elseif plugin == "caching_sha2_password" then
        local stage1 = sha256_of(password)
        return xor(stage1, sha256_of(sha256_of(stage1) .. sub(scramble, 1, 20)))
    elseif plugin == "sha256_password" then
        return nil
    end
    return nil, "auth plugin " .. plugin .. " is not supported"
end

-- The password in clear text, which only a TLS connection may carry.
local function clear_password(self, password, plugin)
    if not self.ssl then
        return nil,
            "auth plugin " .. plugin .. " needs ssl to send the password: encrypting it with the "
                .. "server's RSA key is not available"
    end
    return password .. "\0"
end

-- Follows the server through authentication until its OK packet.
local function authenticate(self, plugin, password, scramble)
    while true do
        local payload, err = read_packet(self)
        if not payload then
            return nil, err
        end
        local first = byte(payload)
        if first == 0x00 then
            return true
        elseif first == 0xff then
            return nil, error_of(payload)
        elseif first == 0xfe then
            local name_end = find(payload, "\0", 2, true) or (#payload + 1)
            plugin = sub(payload, 2, name_end - 1)
            scramble = sub(payload, name_end + 1)
            local answer
            answer, err = auth_response(plugin, password, scramble)
            if err then
                return nil, err
            end
            if answer == nil then
                answer, err = clear_password(self, password, plugin)
                if not answer then
                    return nil, err
                end
            end
            local bytes
            bytes, err = send_packet(self, answer)
            if not bytes then
                return nil, err
            end
        elseif first == 0x01 then
            local status = byte(payload, 2)
            if plugin == "caching_sha2_password" and status == 0x04 then
                local answer
                answer, err = clear_password(self, password, plugin)
                if not answer then
                    return nil, err
                end
                local bytes
                bytes, err = send_packet(self, answer)
                if not bytes then
                    return nil, err
                end
            elseif not (plugin == "caching_sha2_password" and status == 0x03) then
                return nil, "unexpected auth data from the server"
            end
        else
            return nil, "bad packet type during authentication: " .. first
        end
    end
end

local function handshake(self, opts)
    local payload, err = read_packet(self)
    if not payload then
        return nil, err
    end
    if byte(payload) == 0xff then
        return nil, error_of(payload)
    end
    if byte(payload) ~= 10 then
        return nil, "unsupported protocol version " .. byte(payload)
    end
    local version_end = find(payload, "\0", 2, true)
    self._server_ver = sub(payload, 2, version_end - 1)
    local at = version_end + 1
    local scramble1 = sub(payload, at + 4, at + 11)
    local lower = unpack("<I2", payload, at + 13)
    local server_charset = byte(payload, at + 15)
    local upper = unpack("<I2", payload, at + 18)
    local capabilities = lower + upper * 65536
    local data_length = byte(payload, at + 20)
    at += 31
    local scramble2_length = math.max(13, data_length - 8)
    local scramble2 = sub(payload, at, at + scramble2_length - 2)
    at += scramble2_length
    local plugin = "mysql_native_password"
    if bit32.band(capabilities, PLUGIN_AUTH) ~= 0 then
        local plugin_end = find(payload, "\0", at, true) or (#payload + 1)
        plugin = sub(payload, at, plugin_end - 1)
    end
    local scramble = scramble1 .. scramble2

    local charset = server_charset
    if opts.charset then
        charset = CHARSETS[opts.charset]
        if not charset then
            return nil, "charset '" .. tostring(opts.charset) .. "' is not supported"
        end
    end
    local flags = CLIENT_FLAGS
    if opts.database then
        flags += CONNECT_WITH_DB
    end
    if opts.ssl then
        if bit32.band(capabilities, SSL) == 0 then
            return nil, "ssl disabled on server"
        end
        flags += SSL
        local bytes
        bytes, err = send_packet(self, pack("<I4I4B", flags, self.max_packet_size, charset) .. string.rep("\0", 23))
        if not bytes then
            return nil, "failed to send client authentication packet: " .. err
        end
        local ok
        ok, err = self.sock:sslhandshake(false, opts.server_name or opts.host, opts.ssl_verify)
        if not ok then
            return nil, "failed to do ssl handshake: " .. (err or "")
        end
        self.ssl = true
    end

    local password = opts.password or ""
    local answer
    answer, err = auth_response(plugin, password, scramble)
    if err then
        return nil, err
    end
    if answer == nil then
        answer, err = clear_password(self, password, plugin)
        if not answer then
            return nil, err
        end
    end
    local parts = {
        pack("<I4I4B", flags, self.max_packet_size, charset),
        string.rep("\0", 23),
        (opts.user or "") .. "\0",
        char(#answer) .. answer,
    }
    if opts.database then
        parts[#parts + 1] = opts.database .. "\0"
    end
    parts[#parts + 1] = plugin .. "\0"
    local bytes
    bytes, err = send_packet(self, concat(parts))
    if not bytes then
        return nil, "failed to send client authentication packet: " .. err
    end
    return authenticate(self, plugin, password, scramble)
end

function mysql.connect(self, opts)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    self.max_packet_size = opts.max_packet_size or 1024 * 1024
    self.compact = opts.compact_arrays
    self.ssl = nil
    local database, user = opts.database or "", opts.user or ""
    local ok, err
    if opts.path then
        ok, err = sock:connect("unix:" .. opts.path, nil, {
            pool = opts.pool or (user .. ":" .. database .. ":" .. opts.path),
        })
    else
        local host, port = opts.host, opts.port or 3306
        ok, err = sock:connect(host, port, {
            pool = opts.pool or (user .. ":" .. database .. ":" .. host .. ":" .. port),
            pool_size = opts.pool_size,
            backlog = opts.backlog,
        })
    end
    if not ok then
        return nil, "failed to connect: " .. err
    end
    if sock:getreusedtimes() > 0 then
        self.state = READY
        return 1
    end
    self.packet_no = -1
    local done, failure, code, sqlstate = handshake(self, opts)
    if not done then
        sock:close()
        return nil, failure, code, sqlstate
    end
    self.state = READY
    return 1
end

function mysql.set_keepalive(self, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if self.state ~= READY then
        return nil, "cannot be reused in the current connection state: " .. (self.state or "nil")
    end
    self.state = nil
    return sock:setkeepalive(...)
end

function mysql.get_reused_times(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:getreusedtimes()
end

function mysql.close(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    self.state = nil
    self.packet_no = -1
    send_packet(self, COM_QUIT)
    return sock:close()
end

function mysql.send_query(self, query)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if self.state ~= READY then
        return nil, "cannot send query in the current context: " .. (self.state or "nil")
    end
    self.packet_no = -1
    local bytes, err = send_packet(self, COM_QUERY .. query)
    if not bytes then
        return nil, err
    end
    self.state = SENT
    return bytes
end

local function column_of(payload)
    local at = 1
    for _ = 1, 4 do
        local _, next_at = lenenc_string(payload, at)
        at = next_at
    end
    local name
    name, at = lenenc_string(payload, at)
    local _, after_org = lenenc_string(payload, at)
    at = after_org + 1
    local kind = byte(payload, at + 6)
    return name, kind
end

local function eof_of(payload)
    return byte(payload) == 0xfe and #payload < 9
end

function mysql.read_result(self, est_nrows)
    if self.state ~= SENT and self.state ~= PENDING then
        return nil, "cannot read result in the current context: " .. (self.state or "nil")
    end
    local payload, err = read_packet(self)
    if not payload then
        self.state = nil
        return nil, err
    end
    local first = byte(payload)
    if first == 0xff then
        self.state = READY
        return nil, error_of(payload)
    end
    if first == 0x00 then
        local result, status = ok_of(payload)
        if bit32.band(status, SERVER_MORE_RESULTS_EXISTS) ~= 0 then
            self.state = PENDING
            return result, "again"
        end
        self.state = READY
        return result
    end
    local count = lenenc(payload, 1)
    local names, kinds = create(count), create(count)
    for index = 1, count do
        local column
        column, err = read_packet(self)
        if not column then
            self.state = nil
            return nil, err
        end
        names[index], kinds[index] = column_of(column)
    end
    payload, err = read_packet(self)
    if not payload then
        self.state = nil
        return nil, err
    end
    if not eof_of(payload) then
        self.state = nil
        return nil, "bad packet after the column definitions"
    end
    local rows = create(est_nrows or 4)
    local compact = self.compact
    while true do
        payload, err = read_packet(self)
        if not payload then
            self.state = nil
            return nil, err
        end
        if eof_of(payload) then
            local status = unpack("<I2", payload, 4)
            if bit32.band(status, SERVER_MORE_RESULTS_EXISTS) ~= 0 then
                self.state = PENDING
                return rows, "again"
            end
            self.state = READY
            return rows
        end
        if byte(payload) == 0xff then
            self.state = READY
            return nil, error_of(payload)
        end
        local row = compact and create(count) or {}
        local at = 1
        for index = 1, count do
            local value
            value, at = lenenc_string(payload, at)
            if value == nil then
                value = ngx.null
            elseif NUMERIC[kinds[index]] then
                value = tonumber(value) or value
            end
            if compact then
                row[index] = value
            else
                row[names[index]] = value
            end
        end
        rows[#rows + 1] = row
    end
end

function mysql.query(self, query, est_nrows)
    local bytes, err = mysql.send_query(self, query)
    if not bytes then
        return nil, "failed to send query: " .. err
    end
    return mysql.read_result(self, est_nrows)
end

return mysql
