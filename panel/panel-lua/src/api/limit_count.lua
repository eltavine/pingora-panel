-- resty.limit.count: lua-resty-limit-traffic's limit on the requests per
-- key in each fixed window of time.
local limit = { _VERSION = "0.09" }
local methods = { __index = limit }

function limit.new(dict_name, limit_count, window)
    local dict = ngx.shared[dict_name]
    if not dict then
        return nil, "shared dict not found"
    end
    assert(limit_count > 0 and window > 0)
    return setmetatable({
        dict = dict,
        limit = limit_count,
        window = window,
    }, methods)
end

-- 0, and how many requests with `key` the window still allows.
function limit.incoming(self, key, commit)
    local remaining, err
    if commit then
        remaining, err = self.dict:incr(key, -1, self.limit, self.window)
        if not remaining then
            return nil, err
        end
    else
        remaining = (self.dict:get(key) or self.limit) - 1
    end
    if remaining < 0 then
        return nil, "rejected"
    end
    return 0, remaining
end

function limit.uncommit(self, key)
    assert(key)
    return self.dict:incr(key, 1)
end

return limit
