-- resty.upstream.healthcheck: lua-resty-upstream-healthcheck's active
-- checks of an upstream's peers, which take them down through
-- ngx.upstream after `fall` failed checks and up again after `rise`
-- passing ones, and its status pages.
local upstream = require "ngx.upstream"
local concat, insert = table.concat, table.insert
local match = string.match

local healthcheck = { _VERSION = "0.08" }

-- Checkers this VM spawned, by upstream.
local checkers = {}

local function peer_key(prefix, name, backup, id)
    return prefix .. name .. (backup and ":b" or ":p") .. id
end

-- The host and port of a peer's name, with `port` instead when given.
local function address(name, port)
    local host, own = match(name, "^%[(.+)%]:(%d+)$")
    if not host then
        host, own = match(name, "^(.+):(%d+)$")
    end
    return host or name, port or tonumber(own) or 80
end

-- Whether the peer answers the check's request with a valid status.
local function probe(opts, peer)
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    sock:settimeout(opts.timeout)
    local host, port = address(peer.name, opts.port)
    local ok
    ok, err = sock:connect(host, port)
    if not ok then
        return nil, "failed to connect: " .. err
    end
    if opts.type == "https" then
        ok, err = sock:sslhandshake(nil, opts.host, opts.ssl_verify)
        if not ok then
            sock:close()
            return nil, "failed to do ssl handshake: " .. err
        end
    end
    local bytes
    bytes, err = sock:send(opts.http_req)
    if not bytes then
        sock:close()
        return nil, "failed to send request: " .. err
    end
    local line
    line, err = sock:receive()
    sock:close()
    if not line then
        return nil, "failed to receive status line: " .. err
    end
    local status = tonumber(match(line, "^HTTP/%d+%.?%d* (%d%d%d)"))
    if not status then
        return nil, "bad status line: " .. line
    end
    if opts.statuses and not opts.statuses[status] then
        return nil, "bad status code: " .. status
    end
    return true
end

-- Counts one check of a peer and turns it down or up when the count says.
local function count(opts, peer, backup, passed, why)
    local dict, name = opts.dict, opts.upstream
    local mine = peer_key(passed and "ok:" or "nok:", name, backup, peer.id)
    local other = peer_key(passed and "nok:" or "ok:", name, backup, peer.id)
    dict:delete(other)
    local streak = dict:incr(mine, 1, 0) or 1
    if passed and peer.down and streak >= opts.rise then
        local ok, err = upstream.set_peer_down(name, backup, peer.id, false)
        if ok then
            ngx.log(ngx.WARN, "healthcheck: peer ", peer.name, " was turned up after ", streak, " success(es)")
        else
            ngx.log(ngx.ERR, "healthcheck: failed to turn up peer ", peer.name, ": ", err)
        end
    elseif not passed and not peer.down and streak >= opts.fall then
        local ok, err = upstream.set_peer_down(name, backup, peer.id, true)
        if ok then
            ngx.log(ngx.WARN, "healthcheck: peer ", peer.name, " was turned down after ", streak,
                " failure(s): ", why)
        else
            ngx.log(ngx.ERR, "healthcheck: failed to turn down peer ", peer.name, ": ", err)
        end
    end
end

local function check_peers(opts, peers, backup)
    local index, total = 0, #peers
    local function worker()
        while index < total do
            index += 1
            local peer = peers[index]
            local passed, why = probe(opts, peer)
            count(opts, peer, backup, passed, why)
        end
    end
    local threads = {}
    for _ = 2, math.min(opts.concurrency, total) do
        insert(threads, ngx.thread.spawn(worker))
    end
    worker()
    for _, thread in ipairs(threads) do
        ngx.thread.wait(thread)
    end
end

local function cycle(premature, opts)
    if premature then
        return
    end
    -- One VM checks each round; the others find the round taken.
    local turn = opts.dict:add("l:" .. opts.upstream, true, math.max(opts.interval / 1000 - 0.001, 0.001))
    if turn then
        for _, backup in ipairs({ false, true }) do
            local peers = backup and upstream.get_backup_peers(opts.upstream)
                or upstream.get_primary_peers(opts.upstream)
            if peers and #peers > 0 then
                check_peers(opts, peers, backup)
            end
        end
    end
    local ok, err = ngx.timer.at(opts.interval / 1000, cycle, opts)
    if not ok and err ~= "process exiting" then
        ngx.log(ngx.ERR, "healthcheck: failed to schedule the next check of ", opts.upstream, ": ", err)
    end
end

function healthcheck.spawn_checker(options)
    local kind = options.type
    if not kind then
        return nil, '"type" option required'
    end
    if kind ~= "http" and kind ~= "https" then
        return nil, 'only "http" and "https" type are supported'
    end
    if not options.http_req then
        return nil, '"http_req" option required'
    end
    if not options.shm then
        return nil, '"shm" option required'
    end
    local dict = ngx.shared[options.shm]
    if not dict then
        return nil, 'shm "' .. tostring(options.shm) .. '" not found'
    end
    local name = options.upstream
    if not name then
        return nil, "no upstream specified"
    end
    local peers, err = upstream.get_primary_peers(name)
    if not peers then
        return nil, "failed to get primary peers: " .. err
    end
    local statuses
    if options.valid_statuses then
        statuses = {}
        for _, status in ipairs(options.valid_statuses) do
            statuses[status] = true
        end
    end
    local opts = {
        type = kind,
        http_req = options.http_req,
        dict = dict,
        upstream = name,
        port = options.port,
        interval = options.interval or 1000,
        timeout = options.timeout or 1000,
        fall = options.fall or 5,
        rise = options.rise or 2,
        statuses = statuses,
        concurrency = math.max(options.concurrency or 1, 1),
        ssl_verify = options.ssl_verify ~= false,
        host = options.host,
    }
    local ok
    ok, err = ngx.timer.at(0, cycle, opts)
    if not ok then
        return nil, "failed to create timer: " .. err
    end
    checkers[name] = (checkers[name] or 0) + 1
    return true
end

local function peers_of(name)
    local primary, err = upstream.get_primary_peers(name)
    if not primary then
        return nil, err
    end
    local backup
    backup, err = upstream.get_backup_peers(name)
    if not backup then
        return nil, err
    end
    return primary, backup
end

function healthcheck.status_page()
    local names = upstream.get_upstreams()
    local lines = {}
    for index, name in ipairs(names) do
        if index > 1 then
            insert(lines, "\n")
        end
        insert(lines, "Upstream " .. name .. (checkers[name] and "" or " (NO checkers)") .. "\n")
        local primary, backup = peers_of(name)
        if not primary then
            return "failed to get peers in upstream " .. name .. ": " .. backup
        end
        for _, tier in ipairs({ { "    Primary Peers\n", primary }, { "    Backup Peers\n", backup } }) do
            insert(lines, tier[1])
            for _, peer in ipairs(tier[2]) do
                insert(lines, "        " .. peer.name .. (peer.down and " DOWN\n" or " UP\n"))
            end
        end
    end
    return concat(lines)
end

function healthcheck.prometheus_status_page()
    local lines = {
        "# HELP nginx_upstream_status_info The running status of nginx upstream\n",
        "# TYPE nginx_upstream_status_info gauge\n",
    }
    local function summary(name, up, down, unknown)
        for _, state in ipairs({ { "UP", up }, { "DOWN", down }, { "UNKNOWN", unknown } }) do
            insert(lines, 'nginx_upstream_status_info{name="' .. name .. '",status="' .. state[1] .. '"} '
                .. state[2] .. "\n")
        end
    end
    for _, name in ipairs(upstream.get_upstreams()) do
        if not checkers[name] then
            summary(name, 0, 0, 1)
            continue
        end
        local primary, backup = peers_of(name)
        if not primary then
            summary(name, 0, 1, 0)
            continue
        end
        local any_up = false
        for _, tier in ipairs({ { "PRIMARY", primary }, { "BACKUP", backup } }) do
            for _, peer in ipairs(tier[2]) do
                local labels = 'nginx_upstream_status_info{name="' .. name .. '",endpoint="' .. peer.name
                    .. '",status="'
                insert(lines, labels .. 'UP",role="' .. tier[1] .. '"} ' .. (peer.down and 0 or 1) .. "\n")
                insert(lines, labels .. 'DOWN",role="' .. tier[1] .. '"} ' .. (peer.down and 1 or 0) .. "\n")
                any_up = any_up or not peer.down
            end
        end
        summary(name, any_up and 1 or 0, any_up and 0 or 1, 0)
    end
    return concat(lines)
end

return healthcheck
