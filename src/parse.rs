use token::{Token, TokenType, bad_token};
use {Ctype, Type, Scope};
use util::roundup;

use std::sync::Mutex;
use std::collections::HashMap;

// Quoted from 9cc
// > This is a recursive-descendent parser which constructs abstract
// > syntax tree from input tokens.
//
// > This parser knows only about BNF of the C grammer and doesn't care
// > about its semantics. Therefore, some invalid expressions, such as
// > `1+2=3`, are accepted by this parser, but that's intentional.
// > Semantic errors are detected in a later pass.
//
lazy_static!{
    static ref ENV: Mutex<Env> = Mutex::new(Env::new(None));
}

#[derive(Debug, Clone)]
pub enum NodeType {
    Num(i32), // Number literal
    Str(String, usize), // String literal, (data, len)
    Ident(String), // Identifier
    Vardef(String, Option<Box<Node>>, Scope), // Variable definition, name = init
    Lvar(Scope), // Variable reference
    Gvar(String, String, usize), // Variable reference, (name, data, len)
    BinOp(TokenType, Box<Node>, Box<Node>), // left-hand, right-hand
    If(Box<Node>, Box<Node>, Option<Box<Node>>), // "if" ( cond ) then "else" els
    Ternary(Box<Node>, Box<Node>, Box<Node>), // cond ? then : els
    For(Box<Node>, Box<Node>, Box<Node>, Box<Node>), // "for" ( init; cond; inc ) body
    DoWhile(Box<Node>, Box<Node>), // do { body } while(cond)
    Addr(Box<Node>), // address-of operator("&"), expr
    Deref(Box<Node>), // pointer dereference ("*"), expr
    Dot(Box<Node>, String, usize), // Struct member accessm, (expr, name, offset)
    Exclamation(Box<Node>), // !, expr
    Neg(Box<Node>), // -
    Return(Box<Node>), // "return", stmt
    Sizeof(Box<Node>), // "sizeof", expr
    Alignof(Box<Node>), // "_Alignof", expr
    Call(String, Vec<Node>), // Function call(name, args)
    // Function definition(name, args, body, stacksize)
    Func(String, Vec<Node>, Box<Node>, usize),
    CompStmt(Vec<Node>), // Compound statement
    VecStmt(Vec<Node>), // For the purpose of assign a value when initializing an array.
    ExprStmt(Box<Node>), // Expression statement
    StmtExpr(Box<Node>), // Statement expression (GNU extn.)
    Null,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub op: NodeType, // Node type
    pub ty: Box<Type>, // C type
}

impl Node {
    pub fn new(op: NodeType) -> Self {
        Self {
            op,
            ty: Box::new(Type::default()),
        }
    }

    pub fn int_ty(val: i32) -> Self {
        Node::new(NodeType::Num(val))
    }

    pub fn new_binop(ty: TokenType, lhs: Node, rhs: Node) -> Self {
        Node::new(NodeType::BinOp(ty, Box::new(lhs), Box::new(rhs)))
    }
}

macro_rules! new_expr(
    ($i:path, $expr:expr) => (
        Node::new($i(Box::new($expr)))
    )
);

impl Type {
    pub fn new(ty: Ctype, size: usize) -> Self {
        Type {
            ty,
            size,
            align: size,
        }
    }

    pub fn void_ty() -> Self {
        Type::new(Ctype::Void, 0)
    }

    pub fn char_ty() -> Self {
        Type::new(Ctype::Char, 1)
    }

    pub fn int_ty() -> Self {
        Type::new(Ctype::Int, 4)
    }

    pub fn ptr_to(base: Box<Type>) -> Self {
        Type::new(Ctype::Ptr(base), 8)
    }

    pub fn ary_of(base: Box<Type>, len: usize) -> Self {
        let align = base.align;
        let size = base.size * len;
        let mut ty = Type::new(Ctype::Ary(base, len), size);
        ty.align = align;
        ty
    }

    pub fn new_struct(mut members: Vec<Node>) -> Self {
        let (off, align) = Self::set_offset(&mut members);
        let mut ty: Type = Type::new(Ctype::Struct(members), align);
        ty.size = roundup(off, align);
        ty
    }

    fn set_offset(members: &mut Vec<Node>) -> (usize, usize) {
        let mut off = 0;
        let mut align = 0;
        for node in members {
            if let NodeType::Vardef(_, _, Scope::Local(offset)) = &mut node.op {
                let t = &node.ty;
                off = roundup(off, t.align);
                *offset = off;
                off += t.size;

                if align < t.align {
                    align = t.align;
                }
            } else {
                panic!();
            }
        }
        return (off, align);
    }
}

#[derive(Debug, Clone)]
struct Env {
    tags: HashMap<String, Vec<Node>>,
    typedefs: HashMap<String, Type>,
    next: Option<Box<Env>>,
}

impl Env {
    pub fn new(next: Option<Box<Env>>) -> Self {
        Env {
            next,
            tags: HashMap::new(),
            typedefs: HashMap::new(),
        }
    }
}

fn expect(ty: TokenType, tokens: &Vec<Token>, pos: &mut usize) {
    let t = &tokens[*pos];
    if t.ty != ty {
        bad_token(t, &format!("{:?} expected", ty));
    }
    *pos += 1;
}

fn consume(ty: TokenType, tokens: &Vec<Token>, pos: &mut usize) -> bool {
    let t = &tokens[*pos];
    if t.ty != ty {
        return false;
    }
    *pos += 1;
    return true;
}

fn is_typename(t: &Token) -> bool {
    use self::TokenType::*;
    if let TokenType::Ident(ref name) = t.ty {
        return ENV.lock().unwrap().typedefs.get(name).is_some();
    }
    t.ty == Int || t.ty == Char || t.ty == Void || t.ty == Struct
}

fn read_type(t: &Token, tokens: &Vec<Token>, pos: &mut usize) -> Option<Type> {
    *pos += 1;
    match t.ty {
        TokenType::Ident(ref name) => {
            if let Some(ty) = ENV.lock().unwrap().typedefs.get(name) {
                return Some(ty.clone());
            } else {
                *pos -= 1;
                return None;
            }
        }
        TokenType::Int => Some(Type::int_ty()),
        TokenType::Char => Some(Type::char_ty()),
        TokenType::Void => Some(Type::void_ty()),
        TokenType::Struct => {
            let mut tag_may: Option<String> = None;
            let t = &tokens[*pos];
            if let TokenType::Ident(ref name) = t.ty {
                *pos += 1;
                tag_may = Some(name.clone())
            }

            let mut members = vec![];
            if consume(TokenType::LeftBrace, tokens, pos) {
                while !consume(TokenType::RightBrace, tokens, pos) {
                    members.push(decl(tokens, pos))
                }
            }

            if let Some(tag) = tag_may {
                if members.is_empty() {
                    if let Some(members2) = ENV.lock().unwrap().tags.get(&tag) {
                        members = members2.to_vec();
                        if members.is_empty() {
                            panic!("incomplete type: {}", tag);
                        }
                    }
                } else {
                    ENV.lock().unwrap().tags.insert(tag, members.clone());
                }
            } else {
                if members.is_empty() {
                    panic!("bad struct definition");
                }
            }

            Some(Type::new_struct(members))
        }
        _ => {
            *pos -= 1;
            None
        }
    }
}

fn ident(tokens: &Vec<Token>, pos: &mut usize) -> String {
    let t = &tokens[*pos];
    if let TokenType::Ident(ref name) = t.ty {
        *pos += 1;
        name.clone()
    } else {
        bad_token(t, "variable name expected");
    }
}

fn primary(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let t = &tokens[*pos];
    *pos += 1;
    match t.ty {
        TokenType::Num(val) => {
            let mut node = Node::new(NodeType::Num(val));
            node.ty = Box::new(Type::int_ty());
            node
        }
        TokenType::Str(ref str, len) => {
            let mut node = Node::new(NodeType::Str(str.clone(), len));
            node.ty = Box::new(Type::ary_of(Box::new(Type::char_ty()), str.len()));
            node
        }
        TokenType::Ident(ref name) => {
            if !consume(TokenType::LeftParen, tokens, pos) {
                return Node::new(NodeType::Ident(name.clone()));
            }

            let mut args = vec![];
            if consume(TokenType::RightParen, tokens, pos) {
                return Node::new(NodeType::Call(name.clone(), args));
            }

            args.push(assign(tokens, pos));
            while consume(TokenType::Comma, tokens, pos) {
                args.push(assign(tokens, pos));
            }
            expect(TokenType::RightParen, tokens, pos);
            return Node::new(NodeType::Call(name.clone(), args));
        }
        TokenType::LeftParen => {
            if consume(TokenType::LeftBrace, tokens, pos) {
                let stmt = Box::new(compound_stmt(tokens, pos));
                expect(TokenType::RightParen, tokens, pos);
                return Node::new(NodeType::StmtExpr(stmt));
            }
            let node = expr(tokens, pos);
            expect(TokenType::RightParen, tokens, pos);
            node
        }
        _ => bad_token(t, "number expected"),
    }
}

fn postfix(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = primary(tokens, pos);

    loop {
        if consume(TokenType::Dot, tokens, pos) {
            lhs = Node::new(NodeType::Dot(Box::new(lhs), ident(tokens, pos), 0));
            continue;
        }

        if consume(TokenType::Arrow, tokens, pos) {
            lhs = Node::new(NodeType::Dot(
                Box::new(new_expr!(NodeType::Deref, lhs)),
                ident(tokens, pos),
                0,
            ));
            continue;
        }

        if consume(TokenType::LeftBracket, tokens, pos) {
            lhs = new_expr!(
                NodeType::Deref,
                Node::new_binop(TokenType::Plus, lhs, assign(tokens, pos))
            );
            expect(TokenType::RightBracket, tokens, pos);
            continue;
        }
        return lhs;
    }
}

fn unary(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    if consume(TokenType::Minus, tokens, pos) {
        return new_expr!(NodeType::Neg, unary(tokens, pos));
    }
    if consume(TokenType::Mul, tokens, pos) {
        return new_expr!(NodeType::Deref, unary(tokens, pos));
    }
    if consume(TokenType::And, tokens, pos) {
        return new_expr!(NodeType::Addr, unary(tokens, pos));
    }
    if consume(TokenType::Exclamation, tokens, pos) {
        return new_expr!(NodeType::Exclamation, unary(tokens, pos));
    }
    if consume(TokenType::Sizeof, tokens, pos) {
        return new_expr!(NodeType::Sizeof, unary(tokens, pos));
    }
    if consume(TokenType::Alignof, tokens, pos) {
        return new_expr!(NodeType::Alignof, unary(tokens, pos));
    }
    postfix(tokens, pos)
}

fn mul(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = unary(&tokens, pos);

    loop {
        if consume(TokenType::Mul, tokens, pos) {
            lhs = Node::new_binop(TokenType::Mul, lhs, unary(&tokens, pos));
        } else if consume(TokenType::Div, tokens, pos) {
            lhs = Node::new_binop(TokenType::Div, lhs, unary(&tokens, pos));
        } else if consume(TokenType::Mod, tokens, pos) {
            lhs = Node::new_binop(TokenType::Mod, lhs, unary(&tokens, pos));
        } else {
            return lhs;
        }
    }
}

fn add(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = mul(&tokens, pos);

    loop {
        if consume(TokenType::Plus, tokens, pos) {
            lhs = Node::new_binop(TokenType::Plus, lhs, mul(&tokens, pos));
        } else if consume(TokenType::Minus, tokens, pos) {
            lhs = Node::new_binop(TokenType::Minus, lhs, mul(&tokens, pos));
        } else {
            return lhs;
        }
    }
}

fn shift(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = add(tokens, pos);
    loop {
        if consume(TokenType::SHL, tokens, pos) {
            lhs = Node::new_binop(TokenType::SHL, lhs, add(tokens, pos));
        } else if consume(TokenType::SHR, tokens, pos) {
            lhs = Node::new_binop(TokenType::SHR, lhs, add(tokens, pos));
        } else {
            return lhs;
        }
    }
}

fn relational(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = shift(tokens, pos);
    loop {
        if consume(TokenType::LeftAngleBracket, tokens, pos) {
            lhs = Node::new_binop(TokenType::LeftAngleBracket, lhs, shift(tokens, pos));
        } else if consume(TokenType::RightAngleBracket, tokens, pos) {
            lhs = Node::new_binop(TokenType::LeftAngleBracket, shift(tokens, pos), lhs);
        } else if consume(TokenType::LE, tokens, pos) {
            lhs = Node::new_binop(TokenType::LE, lhs, shift(tokens, pos))
        } else if consume(TokenType::GE, tokens, pos) {
            lhs = Node::new_binop(TokenType::LE, shift(tokens, pos), lhs);
        } else {
            return lhs;
        }
    }
}

fn equality(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = relational(tokens, pos);
    loop {
        if consume(TokenType::EQ, tokens, pos) {
            lhs = Node::new_binop(TokenType::EQ, lhs, relational(tokens, pos));
        } else if consume(TokenType::NE, tokens, pos) {
            lhs = Node::new_binop(TokenType::NE, lhs, relational(tokens, pos));
        } else {
            return lhs;
        }
    }
}

fn bit_and(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = equality(tokens, pos);
    while consume(TokenType::And, tokens, pos) {
        lhs = Node::new_binop(TokenType::And, lhs, equality(tokens, pos));
    }
    return lhs;
}

fn bit_xor(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = bit_and(tokens, pos);
    while consume(TokenType::Hat, tokens, pos) {
        lhs = Node::new_binop(TokenType::Hat, lhs, bit_and(tokens, pos));
    }
    return lhs;
}

fn bit_or(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = bit_xor(tokens, pos);
    while consume(TokenType::VerticalBar, tokens, pos) {
        lhs = Node::new_binop(TokenType::VerticalBar, lhs, bit_xor(tokens, pos));
    }
    return lhs;
}

fn logand(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = bit_or(tokens, pos);
    while consume(TokenType::Logand, tokens, pos) {
        lhs = Node::new_binop(TokenType::Logand, lhs, logand(tokens, pos));
    }
    return lhs;
}

fn logor(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut lhs = logand(tokens, pos);
    while consume(TokenType::Logor, tokens, pos) {
        lhs = Node::new_binop(TokenType::Logor, lhs, logand(tokens, pos));
    }
    return lhs;
}

fn conditional(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let cond = logor(tokens, pos);
    if !consume(TokenType::Question, tokens, pos) {
        return cond;
    }
    let then = expr(tokens, pos);
    expect(TokenType::Colon, tokens, pos);
    let els = conditional(tokens, pos);
    Node::new(NodeType::Ternary(
        Box::new(cond),
        Box::new(then),
        Box::new(els),
    ))
}

fn assign(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let lhs = conditional(tokens, pos);
    if !consume(TokenType::Equal, tokens, pos) {
        return lhs;
    }
    return Node::new_binop(TokenType::Equal, lhs, conditional(tokens, pos));
}

fn expr(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let lhs = assign(tokens, pos);
    if !consume(TokenType::Comma, tokens, pos) {
        return lhs;
    }
    return Node::new_binop(TokenType::Comma, lhs, expr(tokens, pos));
}

fn ctype(tokens: &Vec<Token>, pos: &mut usize) -> Type {
    let t = &tokens[*pos];
    if let Some(mut ty) = read_type(t, tokens, pos) {
        while consume(TokenType::Mul, tokens, pos) {
            ty = Type::ptr_to(Box::new(ty));
        }
        ty
    } else {
        bad_token(t, "typename expected");
    }
}

fn read_array(mut ty: Box<Type>, tokens: &Vec<Token>, pos: &mut usize) -> Box<Type> {
    let mut v: Vec<usize> = vec![];
    while consume(TokenType::LeftBracket, tokens, pos) {
        let len = expr(tokens, pos);
        if let NodeType::Num(n) = len.op {
            v.push(n as usize);
            expect(TokenType::RightBracket, tokens, pos);
        } else {
            panic!("number expected");
        }
    }
    for val in v {
        ty = Box::new(Type::ary_of(ty, val));
    }
    ty
}

fn decl(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    // Read the first half of type name (e.g. `int *`).
    let mut ty = Box::new(ctype(tokens, pos));

    // Read an identifier.
    let name = ident(tokens, pos);
    let init: Option<Box<Node>>;

    // Read the second half of type name (e.g. `[3][5]`).
    ty = read_array(ty, tokens, pos);
    if let Ctype::Void = ty.ty {
        panic!("void variable: {}", name);
    }

    // Read an initializer.
    if consume(TokenType::Equal, tokens, pos) {
        // Assign a value when initializing an array.
        if consume(TokenType::LeftBrace, tokens, pos) {
            let mut stmts = vec![];
            let mut ary_decl = Node::new(NodeType::Vardef(name.clone(), None, Scope::Local(0)));
            ary_decl.ty = ty;
            stmts.push(ary_decl);
            let init_ary = array_init_rval(tokens, pos, Node::new(NodeType::Ident(name)));
            expect(TokenType::Semicolon, tokens, pos);
            stmts.push(init_ary);
            return Node::new(NodeType::VecStmt(stmts));
        }

        init = Some(Box::new(assign(tokens, pos)));
    } else {
        init = None
    }
    expect(TokenType::Semicolon, tokens, pos);
    let mut node = Node::new(NodeType::Vardef(name.clone(), init, Scope::Local(0)));
    node.ty = ty;
    node
}

fn array_init_rval(tokens: &Vec<Token>, pos: &mut usize, ident: Node) -> Node {
    let mut init = vec![];
    let mut i = 0;
    loop {
        let val = primary(tokens, pos);
        let node = new_expr!(
            NodeType::Deref,
            Node::new_binop(TokenType::Plus, ident.clone(), Node::new(NodeType::Num(i)))
        );
        init.push(Node::new(NodeType::ExprStmt(
            Box::new(Node::new_binop(TokenType::Equal, node, val)),
        )));
        if !consume(TokenType::Comma, tokens, pos) {
            break;
        }
        i += 1;
    }
    expect(TokenType::RightBrace, tokens, pos);
    return Node::new(NodeType::VecStmt(init));
}

fn param(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let ty = Box::new(ctype(tokens, pos));
    let name = ident(tokens, pos);
    let mut node = Node::new(NodeType::Vardef(name.clone(), None, Scope::Local(0)));
    node.ty = ty;
    node
}

fn expr_stmt(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let expr = expr(tokens, pos);
    let node = new_expr!(NodeType::ExprStmt, expr);
    expect(TokenType::Semicolon, tokens, pos);
    node
}

fn stmt(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    match tokens[*pos].ty {
        TokenType::Typedef => {
            *pos += 1;
            let node = decl(tokens, pos);
            if let NodeType::Vardef(name, _, _) = node.op {
                ENV.lock().unwrap().typedefs.insert(name, *node.ty);
                return Node::new(NodeType::Null);
            } else {
                unreachable!();
            }
        }
        TokenType::Int | TokenType::Char | TokenType::Struct => return decl(tokens, pos),
        TokenType::If => {
            let mut els = None;
            *pos += 1;
            expect(TokenType::LeftParen, tokens, pos);
            let cond = expr(&tokens, pos);
            expect(TokenType::RightParen, tokens, pos);
            let then = stmt(&tokens, pos);
            if consume(TokenType::Else, tokens, pos) {
                els = Some(Box::new(stmt(&tokens, pos)));
            }
            Node::new(NodeType::If(Box::new(cond), Box::new(then), els))
        }
        TokenType::For => {
            *pos += 1;
            expect(TokenType::LeftParen, tokens, pos);
            let init: Box<Node> = if is_typename(&tokens[*pos]) {
                Box::new(decl(tokens, pos))
            } else {
                Box::new(expr_stmt(tokens, pos))
            };
            let cond = Box::new(expr(&tokens, pos));
            expect(TokenType::Semicolon, tokens, pos);
            let inc = Box::new(new_expr!(NodeType::ExprStmt, expr(&tokens, pos)));
            expect(TokenType::RightParen, tokens, pos);
            let body = Box::new(stmt(&tokens, pos));
            Node::new(NodeType::For(init, cond, inc, body))
        }
        TokenType::While => {
            *pos += 1;
            expect(TokenType::LeftParen, tokens, pos);
            let init = Box::new(Node::new(NodeType::Null));
            let inc = Box::new(Node::new(NodeType::Null));
            let cond = Box::new(expr(&tokens, pos));
            expect(TokenType::RightParen, tokens, pos);
            let body = Box::new(stmt(&tokens, pos));
            Node::new(NodeType::For(init, cond, inc, body))
        }
        TokenType::Do => {
            *pos += 1;
            let body = Box::new(stmt(tokens, pos));
            expect(TokenType::While, tokens, pos);
            expect(TokenType::LeftParen, tokens, pos);
            let cond = Box::new(expr(tokens, pos));
            expect(TokenType::RightParen, tokens, pos);
            expect(TokenType::Semicolon, tokens, pos);
            Node::new(NodeType::DoWhile(body, cond))
        }
        TokenType::Return => {
            *pos += 1;
            let expr = expr(&tokens, pos);
            expect(TokenType::Semicolon, tokens, pos);
            Node::new(NodeType::Return(Box::new(expr)))
        }
        TokenType::LeftBrace => {
            *pos += 1;
            let mut stmts = vec![];
            while !consume(TokenType::RightBrace, tokens, pos) {
                stmts.push(stmt(&tokens, pos));
            }
            Node::new(NodeType::CompStmt(stmts))
        }
        TokenType::Semicolon => {
            *pos += 1;
            Node::new(NodeType::Null)
        }
        _ => {
            if is_typename(&tokens[*pos]) {
                return decl(tokens, pos);
            }
            return expr_stmt(tokens, pos);
        }
    }
}

fn compound_stmt(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let mut stmts = vec![];

    let new_env = Env::new(Some(Box::new(ENV.lock().unwrap().clone())));
    *ENV.lock().unwrap() = new_env;
    while !consume(TokenType::RightBrace, tokens, pos) {
        stmts.push(stmt(tokens, pos));
    }
    let next = ENV.lock().unwrap().next.clone();
    *ENV.lock().unwrap() = *next.unwrap();
    Node::new(NodeType::CompStmt(stmts))
}

fn toplevel(tokens: &Vec<Token>, pos: &mut usize) -> Node {
    let is_extern = consume(TokenType::Extern, &tokens, pos);
    let ty = ctype(tokens, pos);
    let t = &tokens[*pos];
    let name: String;
    if let TokenType::Ident(ref name2) = t.ty {
        name = name2.clone();
    } else {
        bad_token(t, "function or variable name expected");
    }
    *pos += 1;

    // Function
    if consume(TokenType::LeftParen, tokens, pos) {
        let mut args = vec![];
        if !consume(TokenType::RightParen, tokens, pos) {
            args.push(param(tokens, pos));
            while consume(TokenType::Comma, tokens, pos) {
                args.push(param(tokens, pos));
            }
            expect(TokenType::RightParen, tokens, pos);
        }

        expect(TokenType::LeftBrace, tokens, pos);
        let body = compound_stmt(tokens, pos);
        return Node::new(NodeType::Func(name, args, Box::new(body), 0));
    }

    // Global variable
    let ty = read_array(Box::new(ty), tokens, pos);
    let mut node;
    if is_extern {
        node = Node::new(NodeType::Vardef(
            name,
            None,
            Scope::Global(String::new(), 0, true),
        ));
    } else {
        node = Node::new(NodeType::Vardef(
            name,
            None,
            Scope::Global(String::new(), ty.size, false),
        ));
    }
    node.ty = ty;
    expect(TokenType::Semicolon, tokens, pos);
    node
}

/* e.g.
 function -> param
+---------+
int main() {     ; +-+                        int   []         2
  int ary[2];    ;   |               +->stmt->decl->read_array->primary
  ary[0]=1;      ;   | compound_stmt-+->stmt->...                ary
  return ary[0]; ;   |               +->stmt->assign->postfix-+->primary
}                ; +-+                  return        []      +->primary
                                                                 0
*/
pub fn parse(tokens: &Vec<Token>) -> Vec<Node> {
    let mut pos = 0;

    let mut v = vec![];
    while tokens.len() != pos {
        v.push(toplevel(tokens, &mut pos))
    }
    v
}
