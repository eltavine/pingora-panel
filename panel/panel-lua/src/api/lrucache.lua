-- resty.lrucache: a cache of at most a given number of items that drops the
-- one used least recently first, with an optional time to live per item.
-- Each VM has its own caches, as each OpenResty worker does.

local now = ngx.now

local Cache = {}
Cache.__index = Cache

local function unlink(node)
    node.prev.next = node.next
    node.next.prev = node.prev
end

local function push_front(head, node)
    node.next = head.next
    node.prev = head
    head.next.prev = node
    head.next = node
end

local function empty()
    local head = {}
    head.next = head
    head.prev = head
    return head
end

local lrucache = { _VERSION = "0.15" }

function lrucache.new(size, _load_factor)
    if type(size) ~= "number" or size < 1 then
        return nil, "size too small"
    end
    return setmetatable({
        size = math.floor(size),
        items = 0,
        nodes = {},
        head = empty(),
    }, Cache)
end

-- The value of `key`, or nil and the value it had once it has expired;
-- the flags it was set with come third.
function Cache:get(key)
    local node = self.nodes[key]
    if node == nil then
        return nil
    end
    unlink(node)
    push_front(self.head, node)
    if node.expire >= 0 and node.expire < now() then
        return nil, node.value, node.flags
    end
    return node.value, nil, node.flags
end

function Cache:set(key, value, ttl, flags)
    local node = self.nodes[key]
    if node then
        unlink(node)
    else
        if self.items >= self.size then
            node = self.head.prev
            unlink(node)
            self.nodes[node.key] = nil
        else
            node = {}
            self.items = self.items + 1
        end
        node.key = key
        self.nodes[key] = node
    end
    node.value = value
    node.expire = (type(ttl) == "number" and ttl > 0) and now() + ttl or -1
    node.flags = (type(flags) == "number" and flags >= 0) and math.floor(flags) or 0
    push_front(self.head, node)
end

function Cache:delete(key)
    local node = self.nodes[key]
    if node == nil then
        return false
    end
    unlink(node)
    self.nodes[key] = nil
    self.items = self.items - 1
    return true
end

function Cache:count()
    return self.items
end

function Cache:capacity()
    return self.size
end

-- The keys from the one used most recently, at most `max_count` of them
-- unless it is 0 or nil, into `res` when it is given.
function Cache:get_keys(max_count, res)
    local limit = (type(max_count) == "number" and max_count > 0) and max_count or self.items
    local keys = res or {}
    local count = 0
    local node = self.head.next
    while node ~= self.head and count < limit do
        count = count + 1
        keys[count] = node.key
        node = node.next
    end
    keys[count + 1] = nil
    return keys
end

function Cache:flush_all()
    self.nodes = {}
    self.items = 0
    self.head = empty()
end

return lrucache
