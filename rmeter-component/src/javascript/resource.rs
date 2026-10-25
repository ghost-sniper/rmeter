//! OS limits are established before the parent sends any JavaScript to the child.
use std::{
    io,
    process::{Child, Command},
};

pub(super) const MEMORY_BYTES: usize = 256 * 1024 * 1024;

#[cfg(windows)]
pub(super) struct Limits {
    _job: std::os::windows::io::OwnedHandle,
}
#[cfg(not(windows))]
pub(super) struct Limits;

pub(super) fn prepare(command: &mut Command) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[cfg(target_os = "linux")]
        // SAFETY: getpid has no preconditions and only reads the caller's PID.
        let parent_pid = unsafe { libc::getpid() };
        // SAFETY: the hook calls only async-signal-safe libc operations before
        // exec; it does not allocate or acquire locks in the forked process.
        unsafe {
            command.pre_exec(move || {
                let memory = libc::rlimit {
                    rlim_cur: MEMORY_BYTES as _,
                    rlim_max: MEMORY_BYTES as _,
                };
                let core = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                let cpu = libc::rlimit {
                    rlim_cur: 65,
                    rlim_max: 65,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                    || libc::setrlimit(libc::RLIMIT_CORE, &core) != 0
                    || libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() != parent_pid {
                        libc::_exit(1);
                    }
                }
                Ok(())
            });
        }
    }
    #[cfg(not(any(windows, unix)))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "JavaScript process limits unsupported on this platform",
    ));
    Ok(())
}

pub(super) fn attach(child: &Child) -> io::Result<Limits> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: null security attributes/name create a private, non-inheritable
        // job. OwnedHandle closes it on every path. All pointers reference live
        // properly initialized structures for the duration of the calls.
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = OwnedHandle::from_raw_handle(raw);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            info.BasicLimitInformation.ActiveProcessLimit = 1;
            info.ProcessMemoryLimit = MEMORY_BYTES;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Limits { _job: job })
        }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        Ok(Limits)
    }
}
