-- Portions adapted from openresty/lua-resty-limit-traffic.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/lib/resty/limit/count.lua
--
-- Modifications: Fixed-window accounting adapted to the panel
-- shared-dictionary API.
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
