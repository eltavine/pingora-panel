-- resty.redis: lua-resty-redis's Redis client on cosockets, with RESP2
-- replies as that library gives them, pipelines, transactions, Pub/Sub
-- and module commands, and its connect options for TLS, pools, AUTH and
-- SELECT.
local concat, create, insert = table.concat, table.create, table.insert
local byte, sub, lower = string.byte, string.sub, string.lower
local null = ngx.null

local redis = { _VERSION = "0.33" }
local methods = { __index = redis }

-- Commands that put a connection in the subscribed state, and those that
-- may leave it.
local SUBSCRIBING = { subscribe = true, psubscribe = true, ssubscribe = true }
local UNSUBSCRIBING = { unsubscribe = true, punsubscribe = true, sunsubscribe = true }

function redis.new(self)
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    return setmetatable({ _sock = sock }, methods)
end

local function socket(self)
    return rawget(self, "_sock")
end

function redis.set_timeout(self, timeout)
    local sock = socket(self)
    if not sock then
        error("not initialized", 2)
    end
    sock:settimeout(timeout)
end

function redis.set_timeouts(self, connect_timeout, send_timeout, read_timeout)
    local sock = socket(self)
    if not sock then
        error("not initialized", 2)
    end
    sock:settimeouts(connect_timeout, send_timeout, read_timeout)
end

-- The request for `args`, a RESP array of bulk strings.
local function request(args, count)
    local parts = create(count * 5 + 1)
    parts[1] = "*" .. count .. "\r\n"
    for index = 1, count do
        local arg = args[index]
        if type(arg) ~= "string" then
            arg = tostring(arg)
        end
        insert(parts, "$")
        insert(parts, #arg)
        insert(parts, "\r\n")
        insert(parts, arg)
        insert(parts, "\r\n")
    end
    return concat(parts)
end

local read

-- One reply, as lua-resty-redis gives it: false and the message for an
-- error, nil and why when the connection failed.
read = function(self, sock)
    local line, err = sock:receive("*l")
    if not line then
        if err == "timeout" and not rawget(self, "_subscribed") then
            sock:close()
        end
        return nil, err
    end
    local kind = byte(line)
    local rest = sub(line, 2)
    if kind == 43 then -- +
        return rest
    elseif kind == 45 then -- -
        return false, rest
    elseif kind == 58 then -- :
        return tonumber(rest)
    elseif kind == 36 then -- $
        local size = tonumber(rest)
        if not size then
            return nil, "bad reply"
        end
        if size < 0 then
            return null
        end
        local data, failed = sock:receive(size + 2)
        if not data then
            return nil, failed
        end
        return sub(data, 1, size)
    elseif kind == 42 then -- *
        local count = tonumber(rest)
        if not count then
            return nil, "bad reply"
        end
        if count < 0 then
            return null
        end
        local values = create(count)
        for index = 1, count do
            local value, failed = read(self, sock)
            if value == false then
                values[index] = { false, failed }
            elseif value == nil then
                return nil, failed
            else
                values[index] = value
            end
        end
        return values
    end
    return nil, "bad reply"
end

-- Follows the transaction a reply to `command` opens or ends.
local function transacting(self, command, reply)
    if reply == nil or reply == false then
        return
    end
    if command == "multi" then
        self._in_transaction = true
    elseif command == "exec" or command == "discard" then
        self._in_transaction = false
    end
end

-- Leaves the subscribed state once a reply says no subscription is left.
local function unsubscribed(self, reply)
    if type(reply) == "table" and UNSUBSCRIBING[reply[1]] and reply[3] == 0 then
        self._subscribed = false
    end
end

local function command(self, name, ...)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    local verb = lower(name)
    if rawget(self, "_subscribed") and not SUBSCRIBING[verb] and not UNSUBSCRIBING[verb] then
        return nil, "subscribed state"
    end
    local args = { name, ... }
    local req = request(args, select("#", ...) + 1)
    local reqs = rawget(self, "_reqs")
    if reqs then
        reqs[#reqs + 1] = req
        self._pipelined[#reqs] = verb
        return
    end
    local bytes, err = sock:send(req)
    if not bytes then
        return nil, err
    end
    local reply, failed = read(self, sock)
    transacting(self, verb, reply)
    if SUBSCRIBING[verb] and type(reply) == "table" then
        self._subscribed = true
    end
    unsubscribed(self, reply)
    return reply, failed
end

-- Any other name is a Redis command, as lua-resty-redis makes it one.
setmetatable(redis, {
    __index = function(class, name)
        local method = function(self, ...)
            return command(self, name, ...)
        end
        rawset(class, name, method)
        return method
    end,
})

function redis.add_commands(...)
    for index = 1, select("#", ...) do
        local name = select(index, ...)
        rawset(redis, name, function(self, ...)
            return command(self, name, ...)
        end)
    end
end

-- `red:<prefix>():<command>(...)` sends `<prefix>.<command>`, for Redis
-- modules such as RedisBloom.
function redis.register_module_prefix(prefix)
    rawset(redis, prefix, function(self)
        return setmetatable({}, {
            __index = function(proxy, name)
                local method = function(_, ...)
                    return command(self, prefix .. "." .. name, ...)
                end
                rawset(proxy, name, method)
                return method
            end,
        })
    end)
end

function redis.connect(self, host, port, opts)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    if type(port) == "table" then
        opts, port = port, nil
    end
    opts = opts or {}
    local db = opts.db
    if db ~= nil then
        db = tonumber(db)
        if not db then
            error("bad db option: a number is needed", 2)
        end
    end
    local password = opts.password
    if password == "" then
        password = nil
    end
    local username = password and opts.username ~= "" and opts.username or nil
    local pool = opts.pool
    if not pool then
        pool = port and (host .. ":" .. port) or host
        if db then
            pool ..= "/db=" .. db
        end
        if username then
            pool ..= "/user=" .. username
        end
    end
    self._subscribed = false
    self._in_transaction = false
    self._reqs = nil
    local ok, err = sock:connect(host, port, { pool = pool, pool_size = opts.pool_size, backlog = opts.backlog })
    if not ok then
        return nil, err
    end
    if opts.tcp_keepalive then
        ok, err = sock:setoption("keepalive", true)
        if not ok then
            sock:close()
            return nil, "failed to enable tcp keepalive: " .. tostring(err)
        end
    end
    if sock:getreusedtimes() > 0 then
        return 1
    end
    if opts.ssl then
        ok, err = sock:sslhandshake(false, opts.server_name, opts.ssl_verify)
        if not ok then
            return nil, "failed to do ssl handshake: " .. err
        end
    end
    if password then
        local reply
        if username then
            reply, err = command(self, "auth", username, password)
        else
            reply, err = command(self, "auth", password)
        end
        if not reply then
            sock:close()
            return nil, "failed to authenticate: " .. tostring(err)
        end
    end
    if db then
        local reply
        reply, err = command(self, "select", db)
        if not reply then
            sock:close()
            return nil, "failed to select database " .. db .. ": " .. tostring(err)
        end
    end
    return 1
end

function redis.set_keepalive(self, max_idle_timeout, pool_size)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    if rawget(self, "_subscribed") then
        return nil, "subscribed state"
    end
    if rawget(self, "_in_transaction") then
        return nil, "in transaction"
    end
    return sock:setkeepalive(max_idle_timeout, pool_size)
end

function redis.get_reused_times(self)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    return sock:getreusedtimes()
end

function redis.close(self)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    self._subscribed = false
    self._in_transaction = false
    return sock:close()
end

function redis.read_reply(self)
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    if not rawget(self, "_subscribed") then
        return nil, "not subscribed"
    end
    local reply, err = read(self, sock)
    unsubscribed(self, reply)
    return reply, err
end

function redis.init_pipeline(self, n)
    self._reqs = create(n or 4)
    self._pipelined = create(n or 4)
end

function redis.cancel_pipeline(self)
    self._reqs = nil
    self._pipelined = nil
end

function redis.commit_pipeline(self)
    local reqs = rawget(self, "_reqs")
    if not reqs then
        return nil, "no pipeline"
    end
    local pipelined = rawget(self, "_pipelined")
    self._reqs = nil
    self._pipelined = nil
    local sock = socket(self)
    if not sock then
        return nil, "not initialized"
    end
    local bytes, err = sock:send(concat(reqs))
    if not bytes then
        return nil, err
    end
    local replies = create(#reqs)
    for index = 1, #reqs do
        local reply, failed = read(self, sock)
        if reply == nil then
            return nil, failed
        end
        transacting(self, pipelined[index], reply)
        replies[index] = reply == false and { false, failed } or reply
    end
    return replies
end

function redis.hmset(self, hashname, ...)
    if select("#", ...) == 1 then
        local fields = ...
        if type(fields) ~= "table" then
            error("table expected, got " .. type(fields), 2)
        end
        local args = {}
        for field, value in pairs(fields) do
            args[#args + 1] = field
            args[#args + 1] = value
        end
        return command(self, "hmset", hashname, unpack(args))
    end
    return command(self, "hmset", hashname, ...)
end

function redis.hmget(self, hashname, ...)
    if select("#", ...) == 1 and type((...)) == "table" then
        return command(self, "hmget", hashname, unpack((...)))
    end
    return command(self, "hmget", hashname, ...)
end

function redis.array_to_hash(self, array)
    local hash = {}
    for index = 1, #array, 2 do
        hash[array[index]] = array[index + 1]
    end
    return hash
end

return redis
