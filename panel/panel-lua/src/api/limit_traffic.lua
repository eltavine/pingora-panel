-- Portions adapted from openresty/lua-resty-limit-traffic.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/lib/resty/limit/traffic.lua
--
-- Modifications: Limiter combination and rollback adapted for the panel
-- runtime.
--
-- License source:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/README.md#copyright-and-license
--
-- Upstream license: BSD-2-Clause. The following notices apply to the upstream
-- portions; local modifications are covered by the repository LICENSE.
--
-- Copyright (C) 2015-2019, by Yichun "agentzh" Zhang, OpenResty Inc.
--
-- All rights reserved.
--
-- Redistribution and use in source and binary forms, with or without
-- modification, are permitted provided that the following conditions are met:
--
-- * Redistributions of source code must retain the above copyright notice,
-- this list of conditions and the following disclaimer.
--
-- * Redistributions in binary form must reproduce the above copyright notice,
-- this list of conditions and the following disclaimer in the documentation
-- and/or other materials provided with the distribution.
--
-- THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
-- AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
-- IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
-- ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
-- LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
-- CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
-- SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
-- INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
-- CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
-- ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
-- POSSIBILITY OF SUCH DAMAGE.

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
