use port_io::Port;
use spin::Mutex;

const SERIAL_PORT_COM1: u16 = 0x3F8;
const SERIAL_PORT_COM1_READY: u16 = SERIAL_PORT_COM1 + 5;
const SERIAL_PORT_READY_MASK: u8 = 0x20;

static COM1: Port<u8> = Port::new(SERIAL_PORT_COM1);
static COM1_READY: Port<u8> = Port::new(SERIAL_PORT_COM1_READY);


pub fn serial_out(s: &str) {
	for b in s.bytes() {
		wait_for_ready();

		// SAFE because we're just writing to the serial port. 
		// worst-case effects here are simple out-of-order characters in the serial log.
		unsafe { COM1.write(b); }
	}
}


fn wait_for_ready() {
	while COM1_READY.read() & SERIAL_PORT_READY_MASK == 0 {
		// do nothing
	}
}