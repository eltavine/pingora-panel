-- resty.http_headers: lua-resty-http's header tables, whose names match
-- whatever their case (RFC 9110 §5.1) and keep the case they were set in.
local lower = string.lower

local http_headers = { _VERSION = "0.17.2" }

function http_headers.new()
    -- The name each header was set by, by its lowercase form.
    local names = {}
    return setmetatable({}, {
        __index = function(headers, name)
            if type(name) ~= "string" then
                return nil
            end
            local set = names[lower(name)]
            return set and rawget(headers, set)
        end,
        __newindex = function(headers, name, value)
            local key = lower(name)
            local set = names[key]
            if set then
                rawset(headers, set, nil)
            end
            if value == nil then
                names[key] = nil
                return
            end
            names[key] = name
            rawset(headers, name, value)
        end,
    })
end

return http_headers
