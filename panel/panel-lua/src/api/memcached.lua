-- resty.memcached: lua-resty-memcached's client of the memcached text
-- protocol on cosockets, with its replies, key escaping and pipelines.
local concat, insert = table.concat, table.insert
local match, find = string.match, string.find

local memcached = { _VERSION = "0.17" }
local methods = { __index = memcached }

function memcached.new(self, opts)
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    local escape, unescape = ngx.escape_uri, ngx.unescape_uri
    local transform = opts and opts.key_transform
    if transform then
        escape, unescape = transform[1], transform[2]
        if not escape or not unescape then
            return nil, "expecting key_transform = { escape, unescape } table"
        end
    end
    return setmetatable({ sock = sock, escape_key = escape, unescape_key = unescape }, methods)
end

-- A reply line; a timeout leaves the connection unusable, so it is closed.
local function line_of(sock)
    local line, err = sock:receive()
    if not line then
        if err == "timeout" then
            sock:close()
        end
        return nil, err
    end
    return line
end

-- The value of an entry, read after its VALUE line announced `size` bytes.
local function data_of(sock, size)
    local data, err = sock:receive(size)
    if not data then
        return nil, err
    end
    local ending
    ending, err = sock:receive(2)
    if not ending then
        return nil, err
    end
    return data
end

-- Reads VALUE lines until END, into `results` by unescaped key.
local function values(self, sock, with_cas)
    local results = {}
    while true do
        local line, err = line_of(sock)
        if not line then
            return nil, err
        end
        if line == "END" then
            return results
        end
        local key, flags, size, cas = match(line, "^VALUE (%S+) (%d+) (%d+) ?(%d*)$")
        if not key then
            return nil, line
        end
        local data
        data, err = data_of(sock, tonumber(size))
        if not data then
            return nil, err
        end
        results[self.unescape_key(key)] = with_cas and { data, flags, cas } or { data, flags }
    end
end

local function one(self, with_cas)
    return function(sock)
        local results, err = values(self, sock, with_cas)
        if not results then
            if with_cas then
                return nil, nil, nil, err
            end
            return nil, nil, err
        end
        local _, entry = next(results)
        if not entry then
            if with_cas then
                return nil, nil, nil, nil
            end
            return nil, nil, nil
        end
        return entry[1], entry[2], entry[3]
    end
end

-- Sends `request` and reads its reply with `reader`, or queues both while
-- a pipeline is open.
local function exchange(self, request, reader)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local reqs = rawget(self, "_reqs")
    if reqs then
        insert(reqs, request)
        insert(self._readers, reader)
        return 1
    end
    local bytes, err = sock:send(request)
    if not bytes then
        return nil, err
    end
    return reader(sock)
end

local function expect(word)
    return function(sock)
        local line, err = line_of(sock)
        if not line then
            return nil, err
        end
        if line == word then
            return 1
        end
        return nil, line
    end
end

local stored, deleted, touched, okay = expect("STORED"), expect("DELETED"), expect("TOUCHED"), expect("OK")

local function counted(sock)
    local line, err = line_of(sock)
    if not line then
        return nil, err
    end
    if not match(line, "^%d+$") then
        return nil, line
    end
    return line
end

-- A value given as nested tables of strings, joined.
local function flat(value, into)
    if type(value) ~= "table" then
        insert(into, tostring(value))
        return into
    end
    for _, part in ipairs(value) do
        flat(part, into)
    end
    return into
end

local function store(self, command, key, value, exptime, flags, cas)
    local data = concat(flat(value, {}))
    local head = command .. " " .. self.escape_key(key) .. " " .. (flags or 0) .. " " .. (exptime or 0) .. " " .. #data
    if cas then
        head ..= " " .. cas
    end
    return exchange(self, head .. "\r\n" .. data .. "\r\n", stored)
end

for _, command in ipairs({ "set", "add", "replace", "append", "prepend" }) do
    memcached[command] = function(self, key, value, exptime, flags)
        return store(self, command, key, value, exptime, flags)
    end
end

function memcached.cas(self, key, value, cas_unique, exptime, flags)
    return store(self, "cas", key, value, exptime, flags, cas_unique)
end

local function retrieve(self, command, keys, with_cas)
    if type(keys) == "table" then
        if #keys == 0 then
            return {}, nil
        end
        local escaped = {}
        for index, key in ipairs(keys) do
            escaped[index] = self.escape_key(key)
        end
        return exchange(self, command .. " " .. concat(escaped, " ") .. "\r\n", function(sock)
            return values(self, sock, with_cas)
        end)
    end
    return exchange(self, command .. " " .. self.escape_key(keys) .. "\r\n", one(self, with_cas))
end

function memcached.get(self, keys)
    return retrieve(self, "get", keys, false)
end

function memcached.gets(self, keys)
    return retrieve(self, "gets", keys, true)
end

function memcached.delete(self, key)
    return exchange(self, "delete " .. self.escape_key(key) .. "\r\n", deleted)
end

function memcached.incr(self, key, delta)
    return exchange(self, "incr " .. self.escape_key(key) .. " " .. delta .. "\r\n", counted)
end

function memcached.decr(self, key, delta)
    return exchange(self, "decr " .. self.escape_key(key) .. " " .. delta .. "\r\n", counted)
end

function memcached.touch(self, key, exptime)
    return exchange(self, "touch " .. self.escape_key(key) .. " " .. exptime .. "\r\n", touched)
end

function memcached.flush_all(self, time)
    return exchange(self, time and ("flush_all " .. time .. "\r\n") or "flush_all\r\n", okay)
end

function memcached.verbosity(self, level)
    return exchange(self, "verbosity " .. level .. "\r\n", okay)
end

function memcached.version(self)
    return exchange(self, "version\r\n", function(sock)
        local line, err = line_of(sock)
        if not line then
            return nil, err
        end
        local version = match(line, "^VERSION (.+)$")
        if not version then
            return nil, line
        end
        return version
    end)
end

function memcached.stats(self, args)
    return exchange(self, args and ("stats " .. args .. "\r\n") or "stats\r\n", function(sock)
        local lines = {}
        while true do
            local line, err = line_of(sock)
            if not line then
                return nil, err
            end
            if line == "END" then
                return lines
            end
            if line == "ERROR" or find(line, "^%u+_ERROR") then
                return nil, line
            end
            insert(lines, line)
        end
    end)
end

function memcached.quit(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local bytes, err = sock:send("quit\r\n")
    if not bytes then
        return nil, err
    end
    sock:close()
    return 1
end

function memcached.init_pipeline(self, n)
    self._reqs = table.create(n or 4)
    self._readers = table.create(n or 4)
end

function memcached.cancel_pipeline(self)
    self._reqs = nil
    self._readers = nil
end

function memcached.commit_pipeline(self)
    local reqs, readers = rawget(self, "_reqs"), rawget(self, "_readers")
    if not reqs then
        return nil, "no pipeline"
    end
    self._reqs, self._readers = nil, nil
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if #reqs == 0 then
        return nil, "no more cmds"
    end
    local bytes, err = sock:send(reqs)
    if not bytes then
        return nil, err
    end
    local results = table.create(#readers)
    for index, reader in ipairs(readers) do
        results[index] = { reader(sock) }
    end
    return results
end

function memcached.connect(self, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:connect(...)
end

function memcached.sslhandshake(self, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:sslhandshake(...)
end

function memcached.set_timeout(self, timeout)
    return memcached.set_timeouts(self, timeout, timeout, timeout)
end

function memcached.set_timeouts(self, connect_timeout, send_timeout, read_timeout)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    sock:settimeouts(connect_timeout, send_timeout, read_timeout)
    return 1
end

function memcached.set_keepalive(self, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:setkeepalive(...)
end

function memcached.get_reused_times(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:getreusedtimes()
end

function memcached.close(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:close()
end

return memcached
