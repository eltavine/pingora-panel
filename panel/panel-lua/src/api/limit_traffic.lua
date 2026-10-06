-- resty.limit.traffic: lua-resty-limit-traffic's combination of limiters,
-- which commits a request to all of them or to none.
local traffic = { _VERSION = "0.09" }

-- The longest delay the limiters give the request, each with its key of
-- `keys`; `states` receives what each says besides its delay.
function traffic.combine(limiters, keys, states)
    local count = #limiters
    for index = 1, count do
        local delay, err = limiters[index]:incoming(keys[index], false)
        if not delay then
            return nil, err
        end
    end
    local longest = 0
    for index = 1, count do
        local delay, state = limiters[index]:incoming(keys[index], true)
        if not delay then
            for undone = 1, index - 1 do
                limiters[undone]:uncommit(keys[undone])
            end
            return nil, state
        end
        if states then
            states[index] = state
        end
        if delay > longest then
            longest = delay
        end
    end
    return longest
end

return traffic
