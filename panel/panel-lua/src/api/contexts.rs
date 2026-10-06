//! The phases each function is documented for in lua-nginx-module
//! (`**context:**`), with `server_rewrite_by_lua` taking what
//! `rewrite_by_lua` takes.

use crate::exchange::Phase;

const fn bits(phases: &[Phase]) -> u16 {
    let mut bits = 0;
    let mut index = 0;
    while index < phases.len() {
        bits |= phases[index].bit();
        if matches!(phases[index], Phase::Rewrite) {
            bits |= Phase::ServerRewrite.bit();
        }
        // lua-nginx-module allows in precontent_by_lua what it allows in
        // access_by_lua.
        if matches!(phases[index], Phase::Access) {
            bits |= Phase::Precontent.bit();
        }
        index += 1;
    }
    bits
}

use Phase::{
    Access as AC, Balancer as BL, BodyFilter as BF, Content as CT, HeaderFilter as HF,
    InitWorker as IW, Log as LG, Rewrite as RW, Set as ST, Timer as TM,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Api {
    Arg,
    Ctx,
    Eof,
    Exit,
    Flush,
    Header,
    HeadersSent,
    IsSubrequest,
    Output,
    Redirect,
    SendHeaders,
    Sleep,
    Status,
    Var,
    ReqClearHeader,
    ReqDiscardBody,
    ReqGetBodyData,
    ReqGetHeaders,
    ReqGetMethod,
    ReqGetPostArgs,
    ReqGetUriArgs,
    ReqHttpVersion,
    ReqIsInternal,
    ReqRawHeader,
    ReqReadBody,
    ReqSetBodyData,
    ReqSetHeader,
    ReqSetMethod,
    ReqSetUri,
    ReqSetUriArgs,
    ReqStartTime,
    RespGetHeaders,
    Balancer,
    Socket,
    Thread,
    Timer,
    Udp,
    SemaphoreWait,
    ReqInitBody,
    ReqAppendBody,
    ReqFinishBody,
    ReqGetBodyFile,
    Exec,
    ReqSocket,
    RunWorkerThread,
}

impl Api {
    const ALL: [Api; 45] = [
        Api::Arg,
        Api::Ctx,
        Api::Eof,
        Api::Exit,
        Api::Flush,
        Api::Header,
        Api::HeadersSent,
        Api::IsSubrequest,
        Api::Output,
        Api::Redirect,
        Api::SendHeaders,
        Api::Sleep,
        Api::Status,
        Api::Var,
        Api::ReqClearHeader,
        Api::ReqDiscardBody,
        Api::ReqGetBodyData,
        Api::ReqGetHeaders,
        Api::ReqGetMethod,
        Api::ReqGetPostArgs,
        Api::ReqGetUriArgs,
        Api::ReqHttpVersion,
        Api::ReqIsInternal,
        Api::ReqRawHeader,
        Api::ReqReadBody,
        Api::ReqSetBodyData,
        Api::ReqSetHeader,
        Api::ReqSetMethod,
        Api::ReqSetUri,
        Api::ReqSetUriArgs,
        Api::ReqStartTime,
        Api::RespGetHeaders,
        Api::Balancer,
        Api::Socket,
        Api::Thread,
        Api::Timer,
        Api::Udp,
        Api::SemaphoreWait,
        Api::ReqInitBody,
        Api::ReqAppendBody,
        Api::ReqFinishBody,
        Api::ReqGetBodyFile,
        Api::Exec,
        Api::ReqSocket,
        Api::RunWorkerThread,
    ];

    /// The function a path such as `ngx.var.host` or `ngx.req.get_headers`
    /// reaches, if a phase restricts it.
    pub(crate) fn by_path(path: &str) -> Option<Self> {
        let path = match path.strip_prefix("ngx.print") {
            Some(rest) if rest.is_empty() || rest.starts_with('.') => "ngx.say",
            _ => path,
        };
        let path = match path {
            "ngx.socket.connect" | "ngx.socket.stream" => "ngx.socket.tcp",
            path => path,
        };
        Self::ALL
            .into_iter()
            .filter(|api| {
                path.strip_prefix(api.name())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
            })
            .max_by_key(|api| api.name().len())
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Api::Arg => "ngx.arg",
            Api::Ctx => "ngx.ctx",
            Api::Eof => "ngx.eof",
            Api::Exit => "ngx.exit",
            Api::Flush => "ngx.flush",
            Api::Header => "ngx.header",
            Api::HeadersSent => "ngx.headers_sent",
            Api::IsSubrequest => "ngx.is_subrequest",
            Api::Output => "ngx.say",
            Api::Redirect => "ngx.redirect",
            Api::SendHeaders => "ngx.send_headers",
            Api::Sleep => "ngx.sleep",
            Api::Status => "ngx.status",
            Api::Var => "ngx.var",
            Api::ReqClearHeader => "ngx.req.clear_header",
            Api::ReqDiscardBody => "ngx.req.discard_body",
            Api::ReqGetBodyData => "ngx.req.get_body_data",
            Api::ReqGetHeaders => "ngx.req.get_headers",
            Api::ReqGetMethod => "ngx.req.get_method",
            Api::ReqGetPostArgs => "ngx.req.get_post_args",
            Api::ReqGetUriArgs => "ngx.req.get_uri_args",
            Api::ReqHttpVersion => "ngx.req.http_version",
            Api::ReqIsInternal => "ngx.req.is_internal",
            Api::ReqRawHeader => "ngx.req.raw_header",
            Api::ReqReadBody => "ngx.req.read_body",
            Api::ReqSetBodyData => "ngx.req.set_body_data",
            Api::ReqSetHeader => "ngx.req.set_header",
            Api::ReqSetMethod => "ngx.req.set_method",
            Api::ReqSetUri => "ngx.req.set_uri",
            Api::ReqSetUriArgs => "ngx.req.set_uri_args",
            Api::ReqStartTime => "ngx.req.start_time",
            Api::RespGetHeaders => "ngx.resp.get_headers",
            Api::Balancer => "ngx.balancer",
            Api::Socket => "ngx.socket.tcp",
            Api::Thread => "ngx.thread",
            Api::Timer => "ngx.timer",
            Api::Udp => "ngx.socket.udp",
            Api::SemaphoreWait => "ngx.semaphore.wait",
            Api::ReqInitBody => "ngx.req.init_body",
            Api::ReqAppendBody => "ngx.req.append_body",
            Api::ReqFinishBody => "ngx.req.finish_body",
            Api::ReqGetBodyFile => "ngx.req.get_body_file",
            Api::Exec => "ngx.exec",
            Api::ReqSocket => "ngx.req.socket",
            Api::RunWorkerThread => "ngx.run_worker_thread",
        }
    }

    pub(crate) const fn phases(self) -> u16 {
        match self {
            Api::Arg => bits(&[ST, BF]),
            Api::Ctx | Api::Timer => bits(&[IW, ST, RW, AC, CT, BL, HF, BF, LG, TM]),
            Api::Eof | Api::Flush | Api::Output | Api::Redirect | Api::SendHeaders => {
                bits(&[RW, AC, CT])
            }
            Api::ReqReadBody | Api::ReqDiscardBody => bits(&[RW, AC, CT]),
            Api::HeadersSent => bits(&[ST, RW, AC, CT]),
            Api::Exit => bits(&[RW, AC, CT, BL, HF, TM]),
            Api::Header => bits(&[RW, AC, CT, HF, BF, LG]),
            Api::Status | Api::IsSubrequest | Api::ReqIsInternal | Api::ReqStartTime => {
                bits(&[ST, RW, AC, CT, HF, BF, LG])
            }
            Api::Sleep => bits(&[RW, AC, CT, TM]),
            Api::Var => bits(&[ST, RW, AC, CT, BL, HF, BF, LG]),
            Api::ReqClearHeader | Api::ReqSetHeader | Api::ReqSetUri | Api::ReqSetUriArgs => {
                bits(&[ST, RW, AC, CT, HF, BF])
            }
            Api::ReqGetBodyData => bits(&[RW, AC, CT, LG]),
            Api::ReqGetHeaders => bits(&[ST, RW, AC, CT, HF, BF, LG]),
            Api::ReqGetPostArgs => bits(&[RW, AC, CT, HF, BF, LG]),
            Api::ReqGetMethod | Api::ReqGetUriArgs => bits(&[ST, RW, AC, CT, BL, HF, BF, LG]),
            Api::ReqHttpVersion | Api::ReqRawHeader => bits(&[ST, RW, AC, CT, HF, LG]),
            Api::ReqSetBodyData => bits(&[RW, AC, CT, BL]),
            Api::ReqSetMethod => bits(&[ST, RW, AC, CT, HF]),
            Api::RespGetHeaders => bits(&[ST, RW, AC, CT, BL, HF, BF, LG]),
            Api::Balancer => bits(&[BL]),
            Api::Socket | Api::Thread | Api::Udp | Api::SemaphoreWait => bits(&[RW, AC, CT, TM]),
            Api::ReqInitBody | Api::ReqAppendBody | Api::ReqFinishBody => bits(&[ST, RW, AC, CT]),
            Api::ReqGetBodyFile | Api::Exec | Api::ReqSocket | Api::RunWorkerThread => {
                bits(&[RW, AC, CT])
            }
        }
    }

    pub(crate) const fn allows(self, phase: Phase) -> bool {
        self.phases() & phase.bit() != 0
    }
}

/// How lua-nginx-module names a phase in its refusals.
pub(crate) const fn context(phase: Phase) -> &'static str {
    match phase {
        Phase::Init => "init_by_lua*",
        Phase::InitWorker => "init_worker_by_lua*",
        Phase::ServerRewrite => "server_rewrite_by_lua*",
        Phase::Rewrite => "rewrite_by_lua*",
        Phase::Access => "access_by_lua*",
        Phase::Precontent => "precontent_by_lua*",
        Phase::Content => "content_by_lua*",
        Phase::Balancer => "balancer_by_lua*",
        Phase::HeaderFilter => "header_filter_by_lua*",
        Phase::BodyFilter => "body_filter_by_lua*",
        Phase::Log => "log_by_lua*",
        Phase::Timer => "ngx.timer",
        Phase::ExitWorker => "exit_worker_by_lua*",
        Phase::WorkerThread => "ngx.run_worker_thread",
        Phase::Set => "set_by_lua*",
    }
}
