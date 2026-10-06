-- resty.limit.req: lua-resty-limit-traffic's leaky bucket limiting the
-- rate of requests per key, its state of each key two numbers packed into
-- an ngx.shared value instead of the C structure the library writes
-- through the FFI.
local abs, max = math.abs, math.max
local pack, unpack = string.pack, string.unpack

-- Excess and the time it was measured, in milli-requests and
-- milliseconds.
local RECORD = "<dd"

local limit = { _VERSION = "0.09" }
local methods = { __index = limit }

function limit.new(dict_name, rate, burst)
    local dict = ngx.shared[dict_name]
    if not dict then
        return nil, "shared dict not found"
    end
    assert(rate > 0 and burst >= 0)
    return setmetatable({
        dict = dict,
        rate = rate * 1000,
        burst = burst * 1000,
    }, methods)
end

local function read(dict, key)
    local record, err = dict:get(key)
    if record == nil then
        return nil, err
    end
    if type(record) ~= "string" or #record ~= 16 then
        return nil, "shdict abused by other users"
    end
    return unpack(RECORD, record)
end

-- The delay before the request with `key` may go on, and the excess per
-- second it leaves.
function limit.incoming(self, key, commit)
    local dict, rate = self.dict, self.rate
    local now = ngx.now() * 1000
    local excess = 0
    local held, last = read(dict, key)
    if held then
        excess = max(held - rate * abs(now - last) / 1000 + 1000, 0)
        if excess > self.burst then
            return nil, "rejected"
        end
    elseif last then
        return nil, last
    end
    if commit then
        local ok, err = dict:set(key, pack(RECORD, excess, now))
        if not ok then
            return nil, err
        end
    end
    return excess / rate, excess / 1000
end

-- Gives back what the last committed request with `key` added.
function limit.uncommit(self, key)
    assert(key)
    local dict = self.dict
    local excess, last = read(dict, key)
    if not excess then
        return nil, last or "not found"
    end
    return dict:set(key, pack(RECORD, max(excess - 1000, 0), last))
end

function limit.set_rate(self, rate)
    self.rate = rate * 1000
end

function limit.set_burst(self, burst)
    self.burst = burst * 1000
end

return limit
