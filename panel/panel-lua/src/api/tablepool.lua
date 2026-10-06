-- tablepool: lua-tablepool's pools of tables to reuse, each VM keeping its
-- own.
local clear, create = table.clear, table.create

-- The most tables a pool keeps; those released beyond are left to the
-- collector.
local MOST = 200

local pools = {}

local tablepool = { _VERSION = "0.03" }

function tablepool.fetch(tag, narr, nrec)
    local pool = pools[tag]
    if pool and pool.n > 0 then
        local count = pool.n
        local reused = pool[count]
        pool[count] = nil
        pool.n = count - 1
        return reused
    end
    return create(narr or 0)
end

function tablepool.release(tag, obj, noclear)
    if type(obj) ~= "table" then
        error("object empty", 2)
    end
    local pool = pools[tag]
    if not pool then
        pool = { n = 0 }
        pools[tag] = pool
    end
    if pool.n >= MOST then
        return
    end
    if not noclear then
        clear(obj)
    end
    local count = pool.n + 1
    pool[count] = obj
    pool.n = count
end

return tablepool
