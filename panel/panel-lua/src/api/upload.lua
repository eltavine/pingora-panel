-- resty.upload: lua-resty-upload's streaming reader of multipart request
-- bodies (RFC 2046 §5.1, RFC 7578), over the request socket.
local match, lower, sub, find = string.match, string.lower, string.sub, string.find
local insert, concat = table.insert, table.concat

local upload = { _VERSION = "0.11" }
local methods = { __index = upload }

local BEGIN, HEADER, BODY, EOF = 1, 2, 3, 4

-- The boundary the Content-Type header names (RFC 2046 §5.1.1).
local function boundary_of(content_type)
    if type(content_type) == "table" then
        content_type = content_type[1]
    end
    if not content_type then
        return nil, "no Content-Type header"
    end
    local parameters = sub(content_type, (find(content_type, ";", 1, true) or #content_type) + 1)
    for parameter in string.gmatch(parameters, "[^;]+") do
        local name, value = match(parameter, "^%s*([^=%s]+)%s*=%s*(.-)%s*$")
        if name and lower(name) == "boundary" then
            value = match(value, '^"(.*)"$') or value
            if value ~= "" then
                return value
            end
        end
    end
    return nil, "no boundary defined in Content-Type"
end

function upload.new(self, chunk_size, max_line_size, preserve_body)
    local boundary, err = boundary_of(ngx.req.get_headers()["content-type"])
    if not boundary then
        return nil, err
    end
    local sock
    sock, err = ngx.req.socket()
    if not sock then
        return nil, err
    end
    local preamble
    preamble, err = sock:receiveuntil("--" .. boundary)
    if not preamble then
        return nil, err
    end
    local body
    body, err = sock:receiveuntil("\r\n--" .. boundary)
    if not body then
        return nil, err
    end
    return setmetatable({
        sock = sock,
        size = chunk_size or 4096,
        line_size = max_line_size or 512,
        boundary = boundary,
        preamble = preamble,
        body = body,
        state = BEGIN,
        kept = preserve_body and {} or nil,
    }, methods)
end

function upload.set_timeout(self, timeout)
    self.sock:settimeout(timeout)
end

-- Keeps what was read, for the request body `preserve_body` restores.
local function keep(self, ...)
    local kept = self.kept
    if kept then
        for index = 1, select("#", ...) do
            insert(kept, (select(index, ...)))
        end
    end
end

-- Puts back the body as it arrived, once all of it is read.
local function restore(self)
    local kept = self.kept
    if not kept then
        return
    end
    self.kept = nil
    local rest = self.sock:receive("*a")
    if rest then
        insert(kept, rest)
    end
    ngx.req.init_body()
    ngx.req.append_body(concat(kept))
    ngx.req.finish_body()
end

-- What follows a delimiter: "--" ends the body, anything else on the line
-- is padding before the next part's header.
local function delimited(self)
    local line, err = self.sock:receive("*l")
    if not line then
        return nil, err
    end
    keep(self, line, "\r\n")
    if sub(line, 1, 2) == "--" then
        self.state = EOF
        restore(self)
    else
        self.state = HEADER
    end
    return true
end

function upload.read(self)
    local state = self.state
    if state == EOF then
        return "eof"
    end
    if state == BEGIN then
        local skipped, err = self.preamble()
        if not skipped then
            return nil, nil, err
        end
        keep(self, skipped, "--", self.boundary)
        local ok, failed = delimited(self)
        if not ok then
            return nil, nil, failed
        end
        if self.state == EOF then
            return "eof"
        end
        state = HEADER
    end
    if state == HEADER then
        local line, err = self.sock:receive("*l")
        if not line then
            return nil, nil, err
        end
        if #line > self.line_size then
            return nil, nil, "line too long: " .. sub(line, 1, 64) .. "..."
        end
        keep(self, line, "\r\n")
        if line == "" then
            self.state = BODY
            return self:read()
        end
        local name, value = match(line, "^([^:%s]+)%s*:%s*(.-)%s*$")
        if not name then
            return "header", line
        end
        return "header", { name, value, line }
    end
    while true do
        local data, err = self.body(self.size)
        if err then
            return nil, nil, err
        end
        if not data then
            keep(self, "\r\n--", self.boundary)
            local ok, failed = delimited(self)
            if not ok then
                return nil, nil, failed
            end
            return "part_end"
        end
        if data ~= "" then
            keep(self, data)
            return "body", data
        end
    end
end

return upload
