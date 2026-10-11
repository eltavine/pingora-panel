-- Portions adapted from openresty/lua-resty-limit-traffic.
-- Upstream source references:
-- https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/lib/resty/limit/conn.lua
--
-- Modifications: Concurrent-request accounting and delay calculation adapted
-- for the panel runtime.
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

-- resty.limit.conn: lua-resty-limit-traffic's limit on the requests per
-- key under way at once, which delays those over `max` and refuses those
-- over `max + burst`.
local floor = math.floor

local limit = { _VERSION = "0.09" }
local methods = { __index = limit }

function limit.new(dict_name, max, burst, default_conn_delay)
    local dict = ngx.shared[dict_name]
    if not dict then
        return nil, "shared dict not found"
    end
    assert(max > 0 and burst >= 0 and default_conn_delay > 0)
    return setmetatable({
        dict = dict,
        max = max,
        burst = burst,
        unit_delay = default_conn_delay,
    }, methods)
end

-- The delay before the request with `key` may go on, and how many are
-- under way with it.
function limit.incoming(self, key, commit)
    local dict = self.dict
    local most = self.max
    self.committed = false
    local conn, err
    if commit then
        conn, err = dict:incr(key, 1, 0)
        if not conn then
            return nil, err
        end
        if conn > most + self.burst then
            local _, undone = dict:incr(key, -1)
            if undone then
                return nil, undone
            end
            return nil, "rejected"
        end
        self.committed = true
    else
        conn = (dict:get(key) or 0) + 1
        if conn > most + self.burst then
            return nil, "rejected"
        end
    end
    if conn > most then
        return self.unit_delay * floor((conn - 1) / most), conn
    end
    return 0, conn
end

function limit.is_committed(self)
    return self.committed
end

-- Ends a request `incoming` committed; `req_latency` moves the delay
-- given to those over the limit towards how long requests take.
function limit.leaving(self, key, req_latency)
    assert(key)
    local conn, err = self.dict:incr(key, -1)
    if not conn then
        return nil, err
    end
    if req_latency then
        self.unit_delay = (req_latency + self.unit_delay) / 2
    end
    return conn
end

function limit.uncommit(self, key)
    assert(key)
    return self.dict:incr(key, -1)
end

function limit.set_conn(self, conn)
    self.max = conn
end

function limit.set_burst(self, burst)
    self.burst = burst
end

return limit
