-- panel.v1 (ADR 0039): the gateway's own API, next to ngx. Each function
-- has one meaning wherever it may be called: lists are lists, missing
-- values are nil, and failures are raised as errors rather than returned
-- as sentinel strings. A later version is a new module, so code written
-- against this one keeps working.

local native = ...
local ngx = ngx
local json = require("cjson.safe")
local balancer = require("ngx.balancer")

local function list(value)
	if value == nil then
		return {}
	elseif type(value) == "table" then
		return value
	elseif value == true then
		return { "" }
	end
	return { value }
end

local function first(value)
	if type(value) == "table" then
		return value[1]
	elseif value == true then
		return ""
	end
	return value
end

local function check(value, err)
	if err ~= nil then
		error(err, 3)
	end
	return value
end

local req = {}

function req.method()
	return ngx.req.get_method()
end

-- The decoded path the route was chosen by.
function req.path()
	return ngx.var.uri
end

-- The path and query as the client sent them.
function req.target()
	return ngx.var.request_uri
end

function req.host()
	return ngx.var.host
end

function req.scheme()
	return ngx.var.scheme
end

function req.client()
	return ngx.var.remote_addr
end

function req.id()
	return ngx.var.request_id
end

-- Every query parameter as a list of its values.
function req.query()
	local args = ngx.req.get_uri_args(0)
	local values = {}
	for name, value in args do
		values[name] = list(value)
	end
	return values
end

function req.query_value(name)
	local args = ngx.req.get_uri_args(0)
	return first(args[name])
end

-- Every header field by its lowercase name, as a list of its values.
function req.headers()
	local headers = ngx.req.get_headers(0)
	local values = {}
	for name, value in headers do
		values[string.lower(name)] = list(value)
	end
	return values
end

-- The first value of a header field, whatever the case of its name.
function req.header(name)
	local headers = ngx.req.get_headers(0)
	return first(headers[name])
end

-- Sets a header field; nil removes it.
function req.set_header(name, value)
	if value == nil then
		ngx.req.clear_header(name)
	else
		ngx.req.set_header(name, value)
	end
end

-- The request body, read when first asked for; needs the body permission.
function req.body()
	ngx.req.read_body()
	return ngx.req.get_body_data() or ""
end

function req.set_body(body)
	ngx.req.read_body()
	ngx.req.set_body_data(body)
end

-- Changes the path; with `choose_route`, the route is chosen again.
function req.set_path(path, choose_route)
	ngx.req.set_uri(path, choose_route == true)
end

function req.set_query(values)
	ngx.req.set_uri_args(values)
end

local resp = {}

-- The status, or nil before one is set.
function resp.status()
	local status = ngx.status
	if status == 0 then
		return nil
	end
	return status
end

function resp.set_status(status)
	ngx.status = status
end

function resp.header(name)
	return first(ngx.header[name])
end

-- Sets a response header field; nil removes it.
function resp.set_header(name, value)
	ngx.header[name] = value
end

-- Answers with `status`, an optional body and header fields, and ends the
-- phase.
function resp.send(status, body, headers)
	ngx.status = status
	if headers ~= nil then
		for name, value in headers do
			ngx.header[name] = value
		end
	end
	if body ~= nil then
		ngx.print(body)
	end
	return ngx.exit(status)
end

-- The request's own table, shared by its phases.
local function ctx()
	return ngx.ctx
end

local upstream = {}

-- Sends this try to `host`, an IP address, and `port`; needs the upstream
-- permission.
function upstream.set_peer(host, port)
	check(balancer.set_current_peer(host, port))
end

-- Allows `tries` more tries after this one fails.
function upstream.retry(tries)
	check(balancer.set_more_tries(tries))
end

-- How the previous try ended, `failed` or `next`, and its status, or nil on
-- the first try.
function upstream.last_failure()
	return balancer.get_last_failure()
end

-- Timeouts of this try in seconds; nil keeps the upstream's.
function upstream.set_timeouts(connect, send, read)
	check(balancer.set_timeouts(connect, send, read))
end

local log = {}

local function write(level, message, fields)
	if fields ~= nil then
		message = message .. " " .. (json.encode(fields) or "")
	end
	ngx.log(level, message)
end

function log.debug(message, fields)
	write(ngx.DEBUG, message, fields)
end

function log.info(message, fields)
	write(ngx.INFO, message, fields)
end

function log.warn(message, fields)
	write(ngx.WARN, message, fields)
end

function log.error(message, fields)
	write(ngx.ERR, message, fields)
end

local codec = {}

function codec.encode(value)
	return check(json.encode(value))
end

-- The value, or nil and why the text is not JSON.
function codec.decode(text)
	return json.decode(text)
end

local re = {}

-- The captures of the first match, or nil.
function re.match(subject, pattern, flags)
	return check(ngx.re.match(subject, pattern, flags))
end

-- Where the first match starts and ends, or nil.
function re.find(subject, pattern, flags)
	local from, to, err = ngx.re.find(subject, pattern, flags)
	check(nil, err)
	return from, to
end

-- The subject with every match replaced, and how many there were.
function re.replace(subject, pattern, replacement, flags)
	local result, count, err = ngx.re.gsub(subject, pattern, replacement, flags)
	check(nil, err)
	return result, count
end

function re.split(subject, pattern, flags)
	return check(ngx.re.split(subject, pattern, flags))
end

local time = {}

-- Seconds since the epoch, with milliseconds.
function time.now()
	return ngx.now()
end

-- An HTTP date (RFC 9110 §5.6.7).
function time.http(seconds)
	return ngx.http_time(seconds or ngx.time())
end

-- The seconds an HTTP date names, or nil.
function time.parse_http(text)
	return ngx.parse_http_time(text)
end

-- An RFC 3339 time in UTC.
function time.rfc3339(seconds)
	return os.date("!%Y-%m-%dT%H:%M:%SZ", seconds or ngx.time())
end

local random = {}

-- A random version 4 UUID.
random.uuid = native.uuid

-- `length` random bytes from the operating system.
random.bytes = native.random_bytes

local crypto = {}
crypto.sha256 = native.sha256
crypto.sha512 = native.sha512
crypto.hmac_sha256 = native.hmac_sha256
crypto.hmac_sha512 = native.hmac_sha512
crypto.equal = native.equal
crypto.base64 = native.base64
crypto.unbase64 = native.unbase64
crypto.base64url = native.base64url
crypto.unbase64url = native.unbase64url

return table.freeze({
	version = 1,
	req = table.freeze(req),
	resp = table.freeze(resp),
	ctx = ctx,
	upstream = table.freeze(upstream),
	log = table.freeze(log),
	json = table.freeze(codec),
	re = table.freeze(re),
	time = table.freeze(time),
	random = table.freeze(random),
	crypto = table.freeze(crypto),
})
