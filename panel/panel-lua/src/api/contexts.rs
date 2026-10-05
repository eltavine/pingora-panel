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
        index += 1;
    }
    bits
}

use Phase::{
    Access as AC, Balancer as BL, BodyFilter as BF, Content as CT, HeaderFilter as HF,
    InitWorker as IW, Log as LG, Rewrite as RW, Timer as TM,
};

const REQUEST: u16 = bits(&[RW, AC, CT, BL, HF, BF, LG, TM]);

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
}

impl Api {
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
        }
    }

    pub(crate) const fn phases(self) -> u16 {
        match self {
            Api::Arg => bits(&[BF]),
            Api::Ctx => bits(&[IW, RW, AC, CT, BL, HF, BF, LG, TM]),
            Api::Eof | Api::Flush | Api::Output | Api::Redirect | Api::SendHeaders => {
                bits(&[RW, AC, CT])
            }
            Api::ReqReadBody | Api::ReqDiscardBody | Api::HeadersSent => bits(&[RW, AC, CT]),
            Api::Exit => bits(&[RW, AC, CT, BL, HF, TM]),
            Api::Header | Api::Status => bits(&[RW, AC, CT, HF, BF, LG]),
            Api::IsSubrequest | Api::ReqIsInternal | Api::ReqStartTime => {
                bits(&[RW, AC, CT, HF, BF, LG])
            }
            Api::Sleep => bits(&[RW, AC, CT, TM]),
            Api::Var => bits(&[RW, AC, CT, BL, HF, BF, LG]),
            Api::ReqClearHeader | Api::ReqSetHeader | Api::ReqSetUri | Api::ReqSetUriArgs => {
                bits(&[RW, AC, CT, HF, BF])
            }
            Api::ReqGetBodyData => bits(&[RW, AC, CT, LG]),
            Api::ReqGetHeaders | Api::ReqGetPostArgs => bits(&[RW, AC, CT, HF, BF, LG]),
            Api::ReqGetMethod | Api::ReqGetUriArgs => bits(&[RW, AC, CT, BL, HF, BF, LG]),
            Api::ReqHttpVersion | Api::ReqRawHeader => bits(&[RW, AC, CT, HF, LG]),
            Api::ReqSetBodyData => bits(&[RW, AC, CT, BL]),
            Api::ReqSetMethod => bits(&[RW, AC, CT, HF]),
            Api::RespGetHeaders => REQUEST,
            Api::Balancer => bits(&[BL]),
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
        Phase::Content => "content_by_lua*",
        Phase::Balancer => "balancer_by_lua*",
        Phase::HeaderFilter => "header_filter_by_lua*",
        Phase::BodyFilter => "body_filter_by_lua*",
        Phase::Log => "log_by_lua*",
        Phase::Timer => "ngx.timer",
    }
}
