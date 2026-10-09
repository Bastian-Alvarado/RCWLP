use std::{fmt, mem, ptr};
#[cfg(feature = "unstable_ds4")]
use std::{marker, pin, thread};
use std::borrow::Borrow;
#[cfg(feature = "unstable_ds4")]
use winapi::shared::winerror;
use crate::*;

/// DualShock4 HID Input report.
#[cfg(feature = "unstable_ds4")]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct DS4Report {
	pub thumb_lx: u8,
	pub thumb_ly: u8,
	pub thumb_rx: u8,
	pub thumb_ry: u8,
	pub buttons: u16,
	pub special: u8,
	pub trigger_l: u8,
	pub trigger_r: u8,
}
#[cfg(feature = "unstable_ds4")]
impl Default for DS4Report {
	#[inline]
	fn default() -> Self {
		DS4Report {
			thumb_lx: 0x80,
			thumb_ly: 0x80,
			thumb_rx: 0x80,
			thumb_ry: 0x80,
			buttons: 0x8,
			special: 0,
			trigger_l: 0,
			trigger_r: 0,
		}
	}
}

// /// DualShock4 v1 complete HID Input report.
// #[derive(Copy, Clone, Debug, Eq, PartialEq)]
// #[repr(C)]
// pub struct DS4ReportEx {
// 	pub thumb_lx: u8,
// 	pub thumb_ly: u8,
// 	pub thumb_rx: u8,
// 	pub thumb_ry: u8,
// 	pub buttons: u16,
// 	pub special: u8,
// 	pub trigger_l: u8,
// 	pub trigger_r: u8,
// 	pub timestamp: u16,
// 	pub battery_lvl: u8,
// 	pub gyro_x: i16,
// 	pub gyro_y: i16,
// 	pub gyro_z: i16,
// 	pub accel_x: i16,
// 	pub accel_y: i16,
// 	pub accel_z: i16,
// 	pub _unknown1: [u8; 5],
// 	pub battery_lvl_special: u8,
// 	pub _unknown2: [u8; 2],
// 	pub touch_packets_n: u8, // 0x00 to 0x03 (USB max)
// 	pub current_touch: DS4Touch,
// 	pub previous_touch: [DS4Touch; 2],
// }

/// A virtual Sony DualShock 4 (wired).
pub struct DualShock4Wired<CL: Borrow<Client>> {
	client: CL,
	event: Event,
	serial_no: u32,
	id: TargetId,
}

impl<CL: Borrow<Client>> DualShock4Wired<CL> {
	/// Creates a new instance.
	#[inline]
	pub fn new(client: CL, id: TargetId) -> DualShock4Wired<CL> {
		let event = Event::new(false, false);
		DualShock4Wired { client, event, serial_no: 0, id }
	}

	/// Returns if the controller is plugged in.
	#[inline]
	pub fn is_attached(&self) -> bool {
		self.serial_no != 0
	}

	/// Returns the id the controller was constructed with.
	#[inline]
	pub fn id(&self) -> TargetId {
		self.id
	}

	/// Returns the client.
	#[inline]
	pub fn client(&self) -> &CL {
		&self.client
	}

	/// Unplugs and destroys the controller, returning the client.
	#[inline]
	pub fn drop(mut self) -> CL {
		let _ = self.unplug();

		unsafe {
			let client = (&self.client as *const CL).read();
			ptr::drop_in_place(&mut self.event);
			mem::forget(self);
			client
		}
	}

	/// Plugs the controller in.
	#[inline(never)]
	pub fn plugin(&mut self) -> Result<(), Error> {
		if self.is_attached() {
			return Err(Error::AlreadyConnected);
		}

		self.serial_no = unsafe {
			let mut plugin = bus::PluginTarget::ds4_wired(1, self.id.vendor, self.id.product);
			let device = self.client.borrow().device;

			// Yes this is how the driver is implemented
			while plugin.ioctl(device, self.event.handle).is_err() {
				plugin.SerialNo += 1;
				if plugin.SerialNo >= u16::MAX as u32 {
					return Err(Error::NoFreeSlot);
				}
			}

			plugin.SerialNo
		};

		Ok(())
	}

	/// Unplugs the controller.
	#[inline(never)]
	pub fn unplug(&mut self) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut unplug = bus::UnplugTarget::new(self.serial_no);
			let device = self.client.borrow().device;
			unplug.ioctl(device, self.event.handle)?;
		}

		self.serial_no = 0;
		Ok(())
	}

	/// Waits until the virtual controller is ready.
	///
	/// Any updates submitted before the virtual controller is ready may return an error.
	#[inline(never)]
	pub fn wait_ready(&mut self) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut wait = bus::WaitDeviceReady::new(self.serial_no);
			let device = self.client.borrow().device;
			wait.ioctl(device, self.event.handle)?;
		}

		Ok(())
	}

	/// Updates the virtual controller state.
	#[cfg(feature = "unstable_ds4")]
	#[inline(never)]
	pub fn update(&mut self, report: &DS4Report) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut dsr = bus::DS4SubmitReport::new(self.serial_no, *report);
			let device = self.client.borrow().device;
			dsr.ioctl(device, self.event.handle)?;
		}

		Ok(())
	}

	/// Submits a full 63-byte DS4 input report (the USB report without its ID byte),
	/// which unlike [`update`](Self::update) can carry gyro, accelerometer and touchpad data.
	#[cfg(feature = "unstable_ds4")]
	#[inline(never)]
	pub fn update_ex(&mut self, report: &[u8; 63]) -> Result<(), Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		unsafe {
			let mut dsr = bus::DS4SubmitReportEx::new(self.serial_no, *report);
			let device = self.client.borrow().device;
			dsr.ioctl(device, self.event.handle)?;
		}

		Ok(())
	}

	/// Requests rumble/lightbar notifications. Create at most one per target.
	#[cfg(feature = "unstable_ds4")]
	#[inline(never)]
	pub fn request_notification(&mut self) -> Result<DS4NotificationRequest, Error> {
		if !self.is_attached() {
			return Err(Error::NotPluggedIn);
		}

		let client = self.client.borrow().try_clone()?;
		let req = bus::RequestNotification::new(bus::DS4RequestNotification::new(self.serial_no));
		Ok(DS4NotificationRequest { client, req, _unpin: marker::PhantomPinned })
	}
}

/// Output sent to the virtual DS4 by a game: rumble and lightbar color.
#[cfg(feature = "unstable_ds4")]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct DS4Notification {
	pub large_motor: u8,
	pub small_motor: u8,
	pub lightbar: (u8, u8, u8),
}

/// Pending DS4 notification request. Mirrors `XRequestNotification`.
#[cfg(feature = "unstable_ds4")]
pub struct DS4NotificationRequest {
	client: Client,
	req: bus::RequestNotification<bus::DS4RequestNotification>,
	_unpin: marker::PhantomPinned,
}

#[cfg(feature = "unstable_ds4")]
impl DS4NotificationRequest {
	/// Spawns a thread that calls `f` for every notification until the target is unplugged.
	pub fn spawn_thread<F: FnMut(DS4Notification) + Send + 'static>(self, mut f: F) -> thread::JoinHandle<()> {
		thread::spawn(move || {
			// Safety: the request is not accessible after it is pinned
			let mut req = self;
			let mut req = unsafe { pin::Pin::new_unchecked(&mut req) };
			loop {
				req.as_mut().request();
				match req.as_mut().poll() {
					Ok(n) => f(n),
					// Aborted when the target is unplugged
					Err(_) => break,
				}
			}
		})
	}

	#[inline(never)]
	fn request(self: pin::Pin<&mut Self>) {
		unsafe {
			let device = self.client.device;
			let req = &mut self.get_unchecked_mut().req;
			if req.buffer.SerialNo != 0 {
				req.ioctl(device);
			}
		}
	}

	/// Blocks until a notification arrives.
	#[inline(never)]
	fn poll(self: pin::Pin<&mut Self>) -> Result<DS4Notification, Error> {
		unsafe {
			let device = self.client.device;
			let req = &mut self.get_unchecked_mut().req;
			match req.poll(device, true) {
				Ok(()) => Ok(DS4Notification {
					large_motor: req.buffer.LargeMotor,
					small_motor: req.buffer.SmallMotor,
					lightbar: (req.buffer.LightbarRed, req.buffer.LightbarGreen, req.buffer.LightbarBlue),
				}),
				Err(winerror::ERROR_OPERATION_ABORTED) => {
					req.buffer.SerialNo = 0;
					Err(Error::OperationAborted)
				},
				Err(err) => Err(Error::WinError(err)),
			}
		}
	}
}

#[cfg(feature = "unstable_ds4")]
unsafe impl Send for DS4NotificationRequest {}

#[cfg(feature = "unstable_ds4")]
impl Drop for DS4NotificationRequest {
	fn drop(&mut self) {
		unsafe {
			let this = pin::Pin::new_unchecked(self);
			if this.req.buffer.SerialNo != 0 {
				let device = this.client.device;
				let req = &mut this.get_unchecked_mut().req;
				let _ = req.cancel(device);
			}
		}
	}
}

impl<CL: Borrow<Client>> fmt::Debug for DualShock4Wired<CL> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("DualShock4Wired")
			.field("serial_no", &self.serial_no)
			.field("vendor_id", &self.id.vendor)
			.field("product_id", &self.id.product)
			.finish()
	}
}

impl<CL: Borrow<Client>> Drop for DualShock4Wired<CL> {
	#[inline]
	fn drop(&mut self) {
		let _ = self.unplug();
	}
}
