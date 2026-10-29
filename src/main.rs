use std::{fs::{self, File, OpenOptions}, io::{self, Read, Write}, mem, os::{fd::AsRawFd, unix::{fs::OpenOptionsExt, net::UnixDatagram}}, path::{Path, PathBuf}, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn terminate(_: i32) { STOP.store(true, Ordering::Relaxed); }
fn check(n: i32) -> io::Result<()> { if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) } }
#[repr(C)]
#[derive(Clone, Copy)]
struct Event { time: libc::timeval, kind: u16, code: u16, value: i32 }
impl Default for Event { fn default() -> Self { Self { time:libc::timeval{tv_sec:0,tv_usec:0},kind:0,code:0,value:0 } } }
struct Mouse(File);
impl Mouse {
    fn new() -> io::Result<Self> {
        let f = OpenOptions::new().write(true).open("/dev/uinput")?;
        let fd = f.as_raw_fd();
        unsafe {
            for bit in [1, 2] { check(libc::ioctl(fd, 0x40045564, bit))?; }
            for button in [272,273] { check(libc::ioctl(fd, 0x40045565, button))?; }
            for bit in [0, 1] { check(libc::ioctl(fd, 0x40045566, bit))?; }
            // struct uinput_setup: input_id, name[80], ff_effects_max.
            let mut setup = [0u8; 92];
            setup[0..2].copy_from_slice(&3u16.to_ne_bytes());
            setup[2..4].copy_from_slice(&0x1234u16.to_ne_bytes());
            setup[4..6].copy_from_slice(&0x5678u16.to_ne_bytes());
            let name = b"Touchmouse relative pointer";
            setup[8..8+name.len()].copy_from_slice(name);
            check(libc::ioctl(fd, 0x405c5503, setup.as_ptr()))?;
            check(libc::ioctl(fd, 0x5501))?;
        }
        Ok(Self(f))
    }
    fn event(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        let e = Event {kind, code, value, ..Event::default()};
        let bytes = unsafe { std::slice::from_raw_parts((&e as *const Event).cast::<u8>(), mem::size_of::<Event>()) };
        self.0.write_all(bytes)
    }
    fn motion(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.event(2, 0, x)?; self.event(2, 1, y)?; self.event(0, 0, 0)
    }
    fn click(&mut self, button: u16) -> io::Result<()> {
        self.event(1, button, 1)?; self.event(0, 0, 0)?;
        self.event(1, button, 0)?; self.event(0, 0, 0)
    }
}
impl Drop for Mouse { fn drop(&mut self) { unsafe {libc::ioctl(self.0.as_raw_fd(), 0x5502);} } }
#[derive(Clone, Copy, Default)]
struct Slot { active: bool, x: Option<i32>, y: Option<i32>, generation: u64 }
struct Gesture {
    previous: Option<(usize,u64,i32,i32)>,
    started: Option<Instant>,
    distance: i64,
    blocked: bool,
    tap_valid: bool,
    max_fingers: usize,
    contacts: Vec<(usize,u64,i32,i32)>,
    remainder: (f64,f64),
}
impl Gesture {
    fn new() -> Self {
        Self {previous:None, started:None, distance:0, blocked:false,
            tap_valid:true, max_fingers:0, contacts:Vec::new(), remainder:(0.,0.)}
    }
    fn frame(&mut self, slots: &[Slot], speed: f64, now: Instant) -> (i32,i32,Option<u16>) {
        // Track each finger independently, including its last coordinates on release.
        // Total movement cancels taps; opposite finger movements cannot cancel out.
        for (idx,generation,x,y) in &mut self.contacts {
            if let Some(s)=slots.get(*idx) {
                if s.generation==*generation {
                    if let (Some(nx),Some(ny))=(s.x,s.y) {
                        self.distance+=(i64::from(nx)-i64::from(*x)).abs()
                            +(i64::from(ny)-i64::from(*y)).abs();
                        *x=nx; *y=ny;
                    }
                }
            }
        }
        let active: Vec<_> = slots.iter().enumerate().filter(|(_,s)|s.active).collect();
        if active.is_empty() {
            let limit=if self.max_fingers==2 {300} else {220};
            let tap=self.tap_valid && self.distance<=12
                && self.started.map(|t|now.duration_since(t)<Duration::from_millis(limit)).unwrap_or(false);
            let button=if tap {match self.max_fingers {1=>Some(272),2=>Some(273),_=>None}} else {None};
            *self=Self::new(); return (0,0,button);
        }
        let started=*self.started.get_or_insert(now);
        for (idx,s) in &active {
            if !self.contacts.iter().any(|c|c.0==*idx && c.1==s.generation) {
                if !self.contacts.is_empty() && now.duration_since(started)>Duration::from_millis(120) {
                    self.tap_valid=false;
                }
                if let (Some(x),Some(y))=(s.x,s.y) {
                    self.contacts.push((*idx,s.generation,x,y));
                } else {self.tap_valid=false;}
            }
        }
        self.max_fingers=self.max_fingers.max(active.len());
        if self.contacts.len()>2 || active.len()>2 {self.tap_valid=false;}
        if active.len()!=1 {self.blocked=true; self.previous=None; return (0,0,None);}
        let (idx,s)=active[0];
        let (Some(x),Some(y))=(s.x,s.y) else {return (0,0,None)};
        let mut result=(0,0,None);
        if let Some((old_idx,generation,px,py))=self.previous {
            if old_idx==idx && generation==s.generation {
                if !self.blocked {
                    self.remainder.0+=f64::from(x-px)*speed;
                    self.remainder.1+=f64::from(y-py)*speed;
                    result.0=self.remainder.0.trunc() as i32;
                    result.1=self.remainder.1.trunc() as i32;
                    self.remainder.0-=f64::from(result.0);
                    self.remainder.1-=f64::from(result.1);
                }
            } else {self.blocked=true; self.tap_valid=false;}
        }
        self.previous=Some((idx,s.generation,x,y)); result
    }
}
fn runtime() -> io::Result<PathBuf> {
    let p=PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(||io::Error::other("XDG_RUNTIME_DIR is required"))?);
    Ok(p.join("touchmouse.sock"))
}
fn device() -> io::Result<PathBuf> {
    for entry in fs::read_dir("/sys/class/input")? {
        let e=entry?; if !e.file_name().to_string_lossy().starts_with("event") {continue;}
        let name=fs::read_to_string(e.path().join("device/name")).unwrap_or_default();
        if name.contains("Goodix Capacitive TouchScreen") {return Ok(Path::new("/dev/input").join(e.file_name()));}
    }
    Err(io::Error::new(io::ErrorKind::NotFound,"Goodix touchscreen not found"))
}
struct SocketCleanup(PathBuf);
impl Drop for SocketCleanup {fn drop(&mut self){let _=fs::remove_file(&self.0);}}
fn run(speed:f64) -> io::Result<()> {
    let path=runtime()?;
    let lock=OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).open(path.with_extension("lock"))?;
    check(unsafe {libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)})?;
    let _=fs::remove_file(&path);
    let socket=UnixDatagram::bind(&path)?; let _cleanup=SocketCleanup(path);
    socket.set_nonblocking(true)?;
    let mut input=OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(device()?)?;
    let mut mouse=Mouse::new()?;
    unsafe {
        libc::signal(libc::SIGTERM,terminate as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT,terminate as *const () as libc::sighandler_t);
        check(libc::ioctl(input.as_raw_fd(),0x40044590,1))?; // EVIOCGRAB, released by closing fd
    }
    let mut slots=[Slot::default();256];
    // MT axes persist across contacts. Seed their current values before reading events.
    for axis in [53,54] {
        let mut values=[0i32;257]; values[0]=axis;
        check(unsafe {libc::ioctl(input.as_raw_fd(),0x8404450a,values.as_mut_ptr())})?;
        for (i,s) in slots.iter_mut().enumerate() {if axis==53 {s.x=Some(values[i+1]);} else {s.y=Some(values[i+1]);}}
    }
    let mut slot=0usize; let mut gesture=Gesture::new();
    eprintln!("Touchmouse active, speed={speed}. Press F9 again to stop.");
    while !STOP.load(Ordering::Relaxed) {
        let mut polls=[libc::pollfd{fd:input.as_raw_fd(),events:libc::POLLIN,revents:0},libc::pollfd{fd:socket.as_raw_fd(),events:libc::POLLIN,revents:0}];
        let n=unsafe {libc::poll(polls.as_mut_ptr(),2,100)};
        if n<0 {if io::Error::last_os_error().kind()==io::ErrorKind::Interrupted {continue;} check(n)?;}
        if polls[1].revents & libc::POLLIN!=0 {let mut b=[0;16]; if socket.recv(&mut b).is_ok_and(|n| &b[..n]==b"stop") {break;}}
        if polls[0].revents & (libc::POLLHUP|libc::POLLERR|libc::POLLNVAL)!=0 {return Err(io::Error::other("touchscreen disconnected"));}
        if polls[0].revents & libc::POLLIN==0 {continue;}
        // Read one complete evdev event per iteration, so stop requests remain responsive.
        let mut e=Event::default();
        let b=unsafe {std::slice::from_raw_parts_mut((&mut e as *mut Event).cast::<u8>(),mem::size_of::<Event>())};
        match input.read(b) { Ok(n) if n==mem::size_of::<Event>()=>{}, Ok(_)=>return Err(io::Error::other("short input event")),Err(e) if e.kind()==io::ErrorKind::WouldBlock=>continue,Err(e)=>return Err(e) }
        match (e.kind,e.code) {
            (3,47)=> {slot=usize::try_from(e.value).ok().filter(|s|*s<slots.len()).ok_or_else(||io::Error::other("unsupported touch slot"))?;},
            (3,57)=> {let s=&mut slots[slot];s.active=e.value>=0;if s.active {s.generation+=1;}},
            (3,53)=>slots[slot].x=Some(e.value), (3,54)=>slots[slot].y=Some(e.value),
            (0,3)=>return Err(io::Error::other("input overrun; stopped safely, restart touchmouse")),
            (0,0)=> {let (x,y,tap)=gesture.frame(&slots,speed,Instant::now());if x!=0||y!=0 {mouse.motion(x,y)?;} if let Some(button)=tap {mouse.click(button)?;}},
            _=>{}
        }
    }
    eprintln!("Touchmouse stopped; touchscreen released."); Ok(())
}
fn main() {
    let args:Vec<_>=std::env::args().collect();
    let result=match args.get(1).map(String::as_str) {
        Some("stop")=>runtime().and_then(|p|{let s=UnixDatagram::unbound()?;s.send_to(b"stop",p)?;Ok(())}),
        Some("run")=>{
            let speed=args.get(2).map(|s|s.parse::<f64>()).unwrap_or(Ok(1.0));
            match speed {Ok(v) if v.is_finite()&&v>0.&&v<=10.=>run(v),_=>Err(io::Error::other("speed must be >0 and <=10"))}
        },
        _=>{eprintln!("Usage: touchmouse run [speed] | stop");std::process::exit(2);}
    };
    if let Err(e)=result {eprintln!("touchmouse: {e}");std::process::exit(1);}
}
#[cfg(test)]
mod tests {
    use super::*;
    fn touch(x:i32,y:i32)->Slot {Slot{active:true,x:Some(x),y:Some(y),generation:1}}
    fn up(mut s:Slot)->Slot {s.active=false;s}
    #[test] fn relative_and_lift() {
        let now=Instant::now();let mut g=Gesture::new();
        assert_eq!(g.frame(&[touch(400,500)],1.,now),(0,0,None));
        assert_eq!(g.frame(&[touch(420,497)],1.,now),(20,-3,None));
        assert_eq!(g.frame(&[],1.,now),(0,0,None));
        assert_eq!(g.frame(&[touch(10,20)],1.,now),(0,0,None));
    }
    #[test] fn tap_and_long_press() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1)],1.,now);
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(100)).2,Some(272));
        g.frame(&[touch(1,1)],1.,now);
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(300)).2,None);
    }
    #[test] fn two_fingers_right_click_once_after_both_lift() {
        let now=Instant::now();let mut g=Gesture::new();
        let a=touch(10,10);let b=touch(500,500);
        g.frame(&[a],1.,now);
        assert_eq!(g.frame(&[a,b],1.,now+Duration::from_millis(40)),(0,0,None));
        assert_eq!(g.frame(&[up(a),b],1.,now+Duration::from_millis(100)),(0,0,None));
        assert_eq!(g.frame(&[up(a),up(b)],1.,now+Duration::from_millis(150)),(0,0,Some(273)));
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(160)).2,None);
    }
    #[test] fn simultaneous_two_finger_tap() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(300,300)],1.,now);
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(100)).2,Some(273));
    }
    #[test] fn two_finger_swipe_never_clicks_or_jumps() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(500,500)],1.,now);
        assert_eq!(g.frame(&[touch(30,1),touch(470,500)],1.,now),(0,0,None));
        assert_eq!(g.frame(&[touch(100,100)],1.,now),(0,0,None));
        assert_eq!(g.frame(&[],1.,now),(0,0,None));
    }
    #[test] fn long_two_finger_press_does_not_click() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(500,500)],1.,now);
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(350)).2,None);
    }
    #[test] fn late_second_finger_does_not_click() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1)],1.,now);
        g.frame(&[touch(1,1),touch(500,500)],1.,now+Duration::from_millis(180));
        assert_eq!(g.frame(&[],1.,now+Duration::from_millis(200)).2,None);
    }
    #[test] fn three_fingers_do_not_click() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(100,100),touch(500,500)],1.,now);
        assert_eq!(g.frame(&[],1.,now).2,None);
    }
    #[test] fn replaced_finger_does_not_click() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(100,100)],1.,now);
        let mut replacement=touch(100,100);replacement.generation=2;
        g.frame(&[touch(1,1),replacement],1.,now);
        assert_eq!(g.frame(&[],1.,now).2,None);
    }
    #[test] fn movement_on_release_cancels_tap() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(1,1),touch(100,100)],1.,now);
        assert_eq!(g.frame(&[up(touch(30,1)),up(touch(100,100))],1.,now).2,None);
    }
    #[test] fn subpixel_accumulates() {
        let now=Instant::now();let mut g=Gesture::new();
        g.frame(&[touch(0,0)],0.5,now);
        assert_eq!(g.frame(&[touch(1,0)],0.5,now).0,0);
        assert_eq!(g.frame(&[touch(2,0)],0.5,now).0,1);
    }
}
