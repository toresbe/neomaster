#include <libusb-1.0/libusb.h>
#include <boost/log/trivial.hpp>
#include <boost/thread.hpp>
#include <iostream>
#include "ui/panel.hpp"
#include <boost/format.hpp>
#include <cerrno>
#include <cstring>
#include <cstdio>

#define VENDOR_ID 0x0cd4
#define PRODUCT_ID 0x1112

/*
void USBPanel::Device::print_status(const USBPanel::Device::bs5_message_t &msg) {
    char bar1[64];
    char bar2[64];
    char bar3[64];
    memset(bar1, '-', sizeof(bar1));
    memset(bar2, '-', sizeof(bar2));
    memset(bar3, '-', sizeof(bar3));
    bar1[this->posWheel1 / 4 % sizeof(bar1)] = '|';
    bar2[this->posWheel2 / 4 % sizeof(bar2)] = '|';
    bar3[msg.b3 / 2] = '|';
    printf("\x1b[0;0H\x1b[2J");
    printf("[%.*s]\n", (int)sizeof(bar1), bar1);
    printf("[%.*s]\t", (int)sizeof(bar2), bar2);
    printf("%s[ <- ]%s[ -> ]%s[ GO ]\x1b[0m\n",  
        (msg.b4 & 0x20 ? "\x1b[7m" : "\x1b[0m"),
        (msg.b4 & 0x10 ? "\x1b[7m" : "\x1b[0m"),
        (msg.b4 & 0x40 ? "\x1b[7m" : "\x1b[0m"));
    printf("[%.*s]\n", (int)sizeof(bar3), bar3);
}
*/
namespace Panel {

	void Input::parse_message(const bs5_message_t &msg) {
		// read new data, figure out the consequence, fire events
		if (msg.wheel_1) {
			this->bs5_state.posWheel1 += msg.wheel_1;
		}
		if (msg.wheel_2) {
			this->bs5_state.posWheel2 += msg.wheel_2;
		}
		if (msg.wheel_3 != this->bs5_state.posWheel3) {
			this->bs5_state.posWheel3 = msg.wheel_3;
		}
		this->bs5_state.button_ok = msg.buttons && 0x40;
		this->bs5_state.button_left = msg.buttons && 0x20;
		this->bs5_state.button_right = msg.buttons && 0x10;
	}

	Input::bs5_damage_t USBDevice::read_status(void) {
		// read status, update internal state, and return a message
		// FIXME: Will not return correct damage map!
		bs5_message_t msg;
		Input::bs5_damage_t damage;
		int actual_length;
		int r = libusb_interrupt_transfer(this->device_handle, 0x81 /*LIBUSB_ENDPOINT_IN*/, (unsigned char *)&msg, sizeof(msg), &actual_length, 0);
		assert(actual_length == 6);
		if (r == 0) {
			// results of the transaction can now be found in the data buffer
			// parse them here and report button press
			damage = {
				msg.wheel_1 ? true : false,
				msg.wheel_2 ? true : false,
				(msg.wheel_3 != this->input.bs5_state.posWheel3) ? true : false,
				(
					(this->input.bs5_state.button_ok != msg.buttons && 0x40) ||
					(this->input.bs5_state.button_left != msg.buttons && 0x20) ||
					(this->input.bs5_state.button_right != msg.buttons && 0x10)
				)
			};
			this->input.parse_message(msg);
		}
		else {
			std::cout << "Eek: " << r << std::strerror(errno) << std::endl;
		}
		return damage;
	}

	USBDevice::USBDevice(void) {
		std::cout << "Object is being created" << std::endl;
		libusb_init(NULL);
		if (open_device()) {
			BOOST_LOG_TRIVIAL(debug) << "Opened BS5 panel device.";
		}
		else {
			throw bs5_panel_exception() << bs5_panel_exception_info("Failed to open BS5 panel device.");
		}
	}

	bool is_beosound5(libusb_device *device) {
		struct libusb_device_descriptor desc;
		libusb_get_device_descriptor(device, &desc);
		if ((desc.idVendor == VENDOR_ID) && (desc.idProduct == PRODUCT_ID))
			return true;
		return false;
	}

	void USBDevice::click() {
		status_flag_t state_with_click = { 0x00, 0x00 };
		memcpy(&state_with_click, &this->status_flags, 2);
		state_with_click[0] |= 0x0F;
		send_status(state_with_click);
	}

	void USBDevice::send_status(status_flag_t status_flags) {
		//foo = libusb_set_configuration(this->device_handle, 0x0200);
		//foo = libusb_set_configuration(this->device_handle, 0x0300); // this means IR enabled
		/*
		*   0x80:   LED solid
		*   0x40:   LCD backlight
		*   0x20:   ?
		*   0x10:   LED blink
		*   0x0x:   Any bits here will trigger a click
		*           seems to make no difference which or how many
		*
		*  Byte 2:
		*   0x80:   Is set when IR receiver is on
		*   0x40:   Is sometimes set when IR receiver is on?
		*
		*/
		//SETTING_CLICK = 0x01
		//uint8_t bar [2] = { 0x00, 0x00 }; // turns off
		//uint8_t bar [2] = { 0x40, 0x00 }; // turns on backlight
		//uint8_t bar [2] = { 0xc0, 0x00 }; // turns on LED
		//uint8_t bar [2] = { 0x80, 0x00 }; // turns off scren, on LED
		//uint8_t bar [2] = { 0xd0, 0x00 }; //  blinking
		int num_bytes;
		num_bytes = libusb_control_transfer(this->device_handle, 0x21, 0x09, 0x0200, 0, status_flags, 2, 1);
		assert(num_bytes == 2);
	}

	void USBDevice::set_backlight(bool is_powered) {
		if (is_powered)
			this->status_flags[0] |= 0x40;
		else
			this->status_flags[0] &= 0xbf;
		send_status(this->status_flags);
	}

	bool USBDevice::open_device() {
		// discover devices
		libusb_device **list;
		libusb_device *found = NULL;
		ssize_t cnt = libusb_get_device_list(NULL, &list);
		ssize_t i = 0;
		int err = 0;
		if (cnt < 0)
			return false;
		for (i = 0; i < cnt; i++) {
			libusb_device *device = list[i];
			if (is_beosound5(device)) {
				found = device;
				break;
			}
		}
		if (found) {
			err = libusb_open(found, &this->device_handle);
			if (err)
				return false;
			else
				return true;
		}
		libusb_free_device_list(list, 1);
		return false;
	}
}
