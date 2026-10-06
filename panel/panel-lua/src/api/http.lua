-- resty.http: lua-resty-http's HTTP/1.x client on cosockets: connections
-- through pools, TLS with client certificates and HTTP proxies, requests
-- with bodies of strings or iterators, responses read whole or streamed
-- in the body's framing (RFC 9112 §6), trailers, pipelines and keepalive.
local http_headers = require "resty.http_headers"
local concat, insert = table.concat, table.insert
local find, format, lower, match, sub, upper = string.find, string.format, string.lower, string.match,
    string.sub, string.upper

local http = { _VERSION = "0.17.2" }
local methods = { __index = http }

local USER_AGENT = "lua-resty-http/" .. http._VERSION .. " (Lua) ngx_lua/" .. tostring(ngx.config.ngx_lua_version)

-- Headers that concern one connection (RFC 9110 §7.6.1), which a proxy
-- does not pass on, and the length the body may no longer have.
local HOP_BY_HOP = {
    connection = true,
    ["keep-alive"] = true,
    ["proxy-authenticate"] = true,
    ["proxy-authorization"] = true,
    te = true,
    trailers = true,
    ["transfer-encoding"] = true,
    upgrade = true,
    ["content-length"] = true,
}

function http.new()
    local sock, err = ngx.socket.tcp()
    if not sock then
        return nil, err
    end
    return setmetatable({ sock = sock, keepalive = true }, methods)
end

function http.set_timeout(self, timeout)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    sock:settimeout(timeout)
    return true
end

function http.set_timeouts(self, connect_timeout, send_timeout, read_timeout)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    sock:settimeouts(connect_timeout, send_timeout, read_timeout)
    return true
end

function http.set_proxy_options(self, opts)
    self.proxy_opts = opts
end

-- The scheme, host, port, path and query of `uri`; a host in brackets is
-- an IPv6 address and keeps them.
function http.parse_uri(_, uri, query_in_path)
    if query_in_path == nil then
        query_in_path = true
    end
    local scheme, rest = match(uri, "^(%a[%w+.-]*):(//.*)$")
    if not scheme then
        rest = match(uri, "^(//.*)$")
        scheme = rest and ngx.var.scheme
    end
    if not rest then
        return nil, "bad uri: " .. uri
    end
    scheme = lower(scheme)
    if scheme ~= "http" and scheme ~= "https" then
        return nil, "bad uri: " .. uri
    end
    local authority, tail = match(rest, "^//([^/?#]*)(.*)$")
    authority = match(authority, "@([^@]*)$") or authority
    local host, port = match(authority, "^(%[[^%]]+%]):?(%d*)$")
    if not host then
        host, port = match(authority, "^([^:]+):?(%d*)$")
    end
    if not host or host == "" then
        return nil, "bad uri: " .. uri
    end
    tail = match(tail, "^([^#]*)")
    local path, query = match(tail, "^([^?]*)%??(.*)$")
    if path == "" then
        path = "/"
    end
    if query_in_path and query ~= "" then
        path, query = path .. "?" .. query, ""
    end
    return { scheme, host, tonumber(port) or (scheme == "https" and 443 or 80), path, query }
end

local function bare(host)
    return match(host, "^%[(.+)%]$") or host
end

-- Whether `no_proxy` (a comma-separated list, or `*`) leaves `host` alone.
local function unproxied(no_proxy, host)
    if not no_proxy then
        return false
    end
    host = lower(bare(host))
    for entry in string.gmatch(lower(no_proxy), "[^,%s]+") do
        if entry == "*" or entry == host then
            return true
        end
        local suffix = sub(entry, 1, 1) == "." and entry or ("." .. entry)
        if sub(host, -#suffix) == suffix then
            return true
        end
    end
    return false
end

-- Opens a tunnel to `host` and `port` through the proxy the socket reached.
local function tunnel(sock, host, port, authorization)
    local authority = (find(host, ":", 1, true) and not find(host, "^%[")) and ("[" .. host .. "]") or host
    local lines = { "CONNECT " .. authority .. ":" .. port .. " HTTP/1.1", "Host: " .. authority .. ":" .. port }
    if authorization then
        insert(lines, "Proxy-Authorization: " .. authorization)
    end
    local bytes, err = sock:send(concat(lines, "\r\n") .. "\r\n\r\n")
    if not bytes then
        return nil, err
    end
    local status_line
    status_line, err = sock:receive("*l")
    if not status_line then
        return nil, err
    end
    local status = tonumber(match(status_line, "^HTTP/%d%.%d (%d%d%d)"))
    repeat
        local line
        line, err = sock:receive("*l")
        if not line then
            return nil, err
        end
    until line == ""
    if status ~= 200 then
        return nil, "failed to establish a tunnel through a proxy: " .. tostring(status)
    end
    return true
end

local function proxy_target(proxy_uri)
    local parsed, err = http.parse_uri(nil, proxy_uri, false)
    if not parsed then
        return nil, err
    end
    if parsed[1] ~= "http" then
        return nil, "only http proxies are supported"
    end
    return bare(parsed[2]), parsed[3]
end

function http.connect(self, options, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if type(options) ~= "table" then
        local host, port, opts = options, ...
        self.host, self.port = host, port
        self.keepalive = true
        if type(port) == "table" then
            port, opts = nil, port
        end
        return sock:connect(host, port, opts)
    end
    local scheme = options.scheme or "http"
    local host = options.host
    if not host then
        return nil, "no host specified"
    end
    local ssl = lower(scheme) == "https"
    local port = options.port or (ssl and 443 or 80)
    local proxy_opts = options.proxy_opts or self.proxy_opts
    local proxy_uri, proxy_authorization
    if proxy_opts and not unproxied(proxy_opts.no_proxy, host) then
        if ssl then
            proxy_uri, proxy_authorization = proxy_opts.https_proxy, proxy_opts.https_proxy_authorization
        else
            proxy_uri, proxy_authorization = proxy_opts.http_proxy, proxy_opts.http_proxy_authorization
        end
    end
    local pool = options.pool
    if not pool then
        local parts = { host, tostring(port) }
        if ssl then
            insert(parts, tostring(options.ssl_server_name))
            insert(parts, tostring(options.ssl_verify ~= false))
            insert(parts, tostring(options.ssl_client_cert))
        end
        if proxy_uri then
            insert(parts, proxy_uri)
            insert(parts, tostring(proxy_authorization))
        end
        pool = concat(parts, ":")
    end
    local connect_options = { pool = pool, pool_size = options.pool_size, backlog = options.backlog }
    local ok, err
    if proxy_uri then
        local proxy_host, proxy_port = proxy_target(proxy_uri)
        if not proxy_host then
            return nil, "failed to parse the proxy uri: " .. proxy_port
        end
        ok, err = sock:connect(proxy_host, proxy_port, connect_options)
        if ok and ssl and sock:getreusedtimes() == 0 then
            ok, err = tunnel(sock, bare(host), port, proxy_authorization)
        end
    else
        ok, err = sock:connect(bare(host), port, connect_options)
    end
    if not ok then
        return nil, err
    end
    self.host, self.port, self.ssl = host, port, ssl
    self.http_proxy = (proxy_uri and not ssl) and { authorization = proxy_authorization } or nil
    self.keepalive = true
    if ssl and sock:getreusedtimes() == 0 then
        if options.ssl_client_cert then
            ok, err = sock:setclientcert(options.ssl_client_cert, options.ssl_client_priv_key)
            if not ok then
                return nil, "failed to set client certificate: " .. err
            end
        end
        ok, err = sock:sslhandshake(
            options.ssl_reused_session,
            options.ssl_server_name or bare(host),
            options.ssl_verify ~= false,
            options.ssl_send_status_req
        )
        if not ok then
            return nil, err
        end
    end
    return true
end

function http.connect_proxy(self, proxy_uri, scheme, host, port, proxy_authorization)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local proxy_host, proxy_port = proxy_target(proxy_uri)
    if not proxy_host then
        return nil, "failed to parse the proxy uri: " .. proxy_port
    end
    local ok, err = sock:connect(proxy_host, proxy_port)
    if not ok then
        return nil, err
    end
    self.host, self.port, self.ssl = host, port, scheme == "https"
    if scheme == "https" then
        return tunnel(sock, bare(host), port, proxy_authorization)
    end
    self.http_proxy = { authorization = proxy_authorization }
    return 1
end

function http.ssl_handshake(self, session, host, verify)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    self.ssl = true
    return sock:sslhandshake(session, host, verify)
end

function http.set_keepalive(self, ...)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    if self.keepalive then
        return sock:setkeepalive(...)
    end
    local ok, err = sock:close()
    if ok then
        return 2, "connection must be closed"
    end
    return ok, err
end

function http.get_reused_times(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:getreusedtimes()
end

function http.close(self)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    return sock:close()
end

local function header_value(value)
    if type(value) == "table" then
        return concat(value, ", ")
    end
    return value
end

-- The connection's Host header: the port only when it is not the scheme's.
local function host_header(self)
    local host, port = self.host or "", self.port
    if find(host, ":", 1, true) and not find(host, "^%[") then
        host = "[" .. host .. "]"
    end
    if port and port ~= (self.ssl and 443 or 80) then
        return host .. ":" .. port
    end
    return host
end

-- Reads a status line and the header fields after it.
local function read_head(sock)
    local line, err = sock:receive("*l")
    if not line then
        return nil, err
    end
    local major, minor, code, reason = match(line, "^HTTP/(%d)%.(%d) (%d%d%d) ?(.*)$")
    if not major then
        return nil, "bad status line: " .. line
    end
    local headers = http_headers.new()
    while true do
        local field
        field, err = sock:receive("*l")
        if not field then
            return nil, err
        end
        if field == "" then
            break
        end
        local name, value = match(field, "^([^:]+):%s*(.-)%s*$")
        if name then
            local previous = headers[name]
            if previous == nil then
                headers[name] = value
            elseif type(previous) == "table" then
                insert(previous, value)
            else
                headers[name] = { previous, value }
            end
        end
    end
    return {
        version = tonumber(major) + tonumber(minor) / 10,
        status = tonumber(code),
        reason = reason,
        headers = headers,
    }
end

local function lists(value, token)
    for item in string.gmatch(lower(header_value(value) or ""), "[^,%s]+") do
        if item == token then
            return true
        end
    end
    return false
end

function http.send_request(self, params)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    params = params or {}
    local method = upper(params.method or "GET")
    local version = params.version or 1.1
    local path = params.path or "/"
    local query = params.query
    if type(query) == "table" then
        query = ngx.encode_args(query)
    end
    local headers = http_headers.new()
    for name, value in pairs(params.headers or {}) do
        headers[name] = value
    end
    local body = params.body
    if type(body) == "table" then
        body = concat(body)
    end
    if type(body) == "string" and headers["Content-Length"] == nil
        and headers["Transfer-Encoding"] == nil then
        headers["Content-Length"] = #body
    elseif body == nil and (method == "POST" or method == "PUT" or method == "PATCH")
        and headers["Content-Length"] == nil and headers["Transfer-Encoding"] == nil then
        headers["Content-Length"] = 0
    end
    if headers["Host"] == nil then
        headers["Host"] = host_header(self)
    end
    if headers["User-Agent"] == nil then
        headers["User-Agent"] = USER_AGENT
    end
    if version == 1.0 and headers["Connection"] == nil then
        headers["Connection"] = "Keep-Alive"
    end
    if self.http_proxy then
        if sub(path, 1, 1) == "/" then
            path = "http://" .. host_header(self) .. path
        end
        if headers["Proxy-Authorization"] == nil and self.http_proxy.authorization then
            headers["Proxy-Authorization"] = self.http_proxy.authorization
        end
    end
    self.keepalive = not lists(headers["Connection"], "close")
        and (version ~= 1.0 or lists(headers["Connection"], "keep-alive"))
    local lines = {
        method .. " " .. path .. ((query and query ~= "") and ("?" .. query) or "") .. " HTTP/" .. format("%.1f", version),
    }
    for name, value in pairs(headers) do
        if type(value) == "table" then
            for _, each in ipairs(value) do
                insert(lines, name .. ": " .. tostring(each))
            end
        else
            insert(lines, name .. ": " .. tostring(value))
        end
    end
    local head = concat(lines, "\r\n") .. "\r\n\r\n"
    local expects = lists(headers["Expect"], "100-continue")
    local bytes, err = sock:send(expects and head or (type(body) == "string" and head .. body or head))
    if not bytes then
        return nil, err
    end
    self.method = method
    self.interim = nil
    if expects then
        local answer
        answer, err = read_head(sock)
        if not answer then
            return nil, err
        end
        if answer.status ~= 100 then
            self.interim = answer
            return true
        end
        if type(body) == "string" then
            bytes, err = sock:send(body)
            if not bytes then
                return nil, err
            end
        end
    end
    if type(body) == "function" then
        while true do
            local chunk, failed = body()
            if failed then
                return nil, failed
            end
            if chunk == nil then
                break
            end
            if chunk ~= "" then
                bytes, err = sock:send(chunk)
                if not bytes then
                    return nil, err
                end
            end
        end
    end
    return true
end

-- The reader of a body framed by chunks, which keeps the trailers for
-- `read_trailers`.
local function chunked_reader(sock, response)
    local left, finished = 0, false
    return function(most)
        if finished then
            return nil
        end
        if left == 0 then
            local line, err = sock:receive("*l")
            if not line then
                return nil, err
            end
            local size = tonumber(match(line, "^%s*(%x+)"), 16)
            if not size then
                return nil, "bad chunk size: " .. line
            end
            if size == 0 then
                local trailers = http_headers.new()
                while true do
                    local field
                    field, err = sock:receive("*l")
                    if not field then
                        return nil, err
                    end
                    if field == "" then
                        break
                    end
                    local name, value = match(field, "^([^:]+):%s*(.-)%s*$")
                    if name then
                        trailers[name] = value
                    end
                end
                response.trailers = trailers
                finished = true
                return nil
            end
            left = size
        end
        local wanted = most and math.min(most, left) or left
        local data, err = sock:receive(wanted)
        if not data then
            return nil, err
        end
        left -= wanted
        if left == 0 then
            local ending
            ending, err = sock:receive(2)
            if not ending then
                return nil, err
            end
        end
        return data
    end
end

local function length_reader(sock, length)
    local left = length
    return function(most)
        if left <= 0 then
            return nil
        end
        local wanted = most and math.min(most, left) or left
        local data, err = sock:receive(wanted)
        if not data then
            return nil, err
        end
        left -= wanted
        return data
    end
end

local function closing_reader(sock)
    local finished = false
    return function(most)
        if finished then
            return nil
        end
        if not most then
            finished = true
            local data, err = sock:receive("*a")
            if not data then
                return nil, err
            end
            return data
        end
        local data, err, partial = sock:receive(most)
        if not data then
            finished = true
            if err == "closed" then
                return partial ~= "" and partial or nil
            end
            return nil, err
        end
        return data
    end
end

local function read_body(response)
    local reader = response.body_reader
    if not reader then
        return nil, "no body to be read"
    end
    local parts = {}
    while true do
        local chunk, err = reader()
        if err then
            return nil, err
        end
        if not chunk then
            break
        end
        insert(parts, chunk)
    end
    return concat(parts)
end

local function read_trailers(response)
    for name, value in pairs(response.trailers or {}) do
        response.headers[name] = value
    end
end

function http.read_response(self, params)
    local sock = self.sock
    if not sock then
        return nil, "not initialized"
    end
    local response, err
    response, self.interim = self.interim, nil
    repeat
        if not response then
            response, err = read_head(sock)
            if not response then
                return nil, err
            end
        end
        local status = response.status
        if status >= 100 and status < 200 and status ~= 101 then
            response = nil
        end
    until response
    local headers, status = response.headers, response.status
    local method = upper((params and params.method) or self.method or "GET")
    if lists(headers["Connection"], "close")
        or (response.version == 1.0 and not lists(headers["Connection"], "keep-alive")) then
        self.keepalive = false
    end
    response.has_body = not (method == "HEAD" or status == 204 or status == 304 or (status >= 100 and status < 200))
    if response.has_body then
        local length = tonumber(header_value(headers["Content-Length"]))
        if lists(headers["Transfer-Encoding"], "chunked") then
            response.body_reader = chunked_reader(sock, response)
        elseif length then
            response.body_reader = length_reader(sock, length)
        else
            self.keepalive = false
            response.body_reader = closing_reader(sock)
        end
    else
        response.body_reader = function()
            return nil
        end
    end
    response.read_body = read_body
    response.read_trailers = read_trailers
    return response
end

function http.request(self, params)
    local ok, err = http.send_request(self, params)
    if not ok then
        return nil, err
    end
    return http.read_response(self, params)
end

-- The reader of a body already read.
local function held_reader(body)
    local at = 1
    return function(most)
        if at > #body then
            return nil
        end
        local last = most and math.min(#body, at + most - 1) or #body
        local data = sub(body, at, last)
        at = last + 1
        return data
    end
end

-- Sends every request, then reads each response with its body in turn:
-- Luau cannot wait inside the metamethod lua-resty-http reads them in
-- when a field is first used. A response that cannot be read is empty.
function http.request_pipeline(self, requests)
    for _, params in ipairs(requests) do
        local ok, err = http.send_request(self, params)
        if not ok then
            return nil, err
        end
    end
    local responses = table.create(#requests)
    for index, params in ipairs(requests) do
        local response = http.read_response(self, params)
        local body = response and response:read_body()
        if body then
            response.body_reader = held_reader(body)
            responses[index] = response
        else
            responses[index] = {}
            for rest = index + 1, #requests do
                responses[rest] = {}
            end
            break
        end
    end
    return responses
end

function http.request_uri(self, uri, params)
    params = params or {}
    local parsed, err = http.parse_uri(self, uri, false)
    if not parsed then
        return nil, err
    end
    local scheme, host, port, path, query = parsed[1], parsed[2], parsed[3], parsed[4], parsed[5]
    local options = {}
    for key, value in pairs(params) do
        options[key] = value
    end
    options.scheme, options.host, options.port = scheme, host, port
    options.path = params.path or path
    options.query = params.query or query
    local ok
    ok, err = http.connect(self, options)
    if not ok then
        return nil, err
    end
    local response
    response, err = http.request(self, options)
    if not response then
        self:close()
        return nil, err
    end
    local body
    body, err = response:read_body()
    if not body then
        self:close()
        return nil, err
    end
    response.body = body
    if params.keepalive == false then
        self:close()
    else
        local kept
        kept, err = self:set_keepalive(params.keepalive_timeout, params.keepalive_pool)
        if not kept then
            return nil, err
        end
    end
    return response
end

function http.get_client_body_reader(_, chunksize, sock)
    chunksize = chunksize or 65536
    if not sock then
        local err
        sock, err = ngx.req.socket()
        if not sock then
            return nil, err
        end
    end
    local finished = false
    return function(most)
        if finished then
            return nil
        end
        local data, err, partial = sock:receive(most or chunksize)
        if not data then
            finished = true
            if err == "closed" then
                return (partial and partial ~= "") and partial or nil
            end
            return nil, err
        end
        return data
    end
end

function http.proxy_request(self, chunksize)
    local headers = ngx.req.get_headers()
    local body
    if headers["content-length"] or headers["transfer-encoding"] then
        local err
        body, err = self:get_client_body_reader(chunksize)
        if not body then
            return nil, err
        end
        headers["transfer-encoding"] = nil
        if not headers["content-length"] then
            headers["transfer-encoding"] = "chunked"
            local plain = body
            local ended = false
            body = function()
                if ended then
                    return nil
                end
                local chunk, failed = plain()
                if failed then
                    return nil, failed
                end
                if not chunk then
                    ended = true
                    return "0\r\n\r\n"
                end
                return format("%x\r\n", #chunk) .. chunk .. "\r\n"
            end
        end
    end
    return self:request({
        method = ngx.req.get_method(),
        path = ngx.var.uri,
        query = ngx.var.args,
        headers = headers,
        body = body,
        version = ngx.req.http_version(),
    })
end

function http.proxy_response(_, response, chunksize)
    if not response then
        ngx.log(ngx.ERR, "no response provided")
        return
    end
    ngx.status = response.status
    for name, value in pairs(response.headers) do
        if not HOP_BY_HOP[lower(name)] then
            ngx.header[name] = value
        end
    end
    local reader = response.body_reader
    repeat
        local chunk, err = reader(chunksize)
        if err then
            ngx.log(ngx.ERR, err)
            break
        end
        if chunk then
            local ok
            ok, err = ngx.print(chunk)
            if not ok then
                ngx.log(ngx.ERR, err)
                break
            end
        end
    until not chunk
end

return http
