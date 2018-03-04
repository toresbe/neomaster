#include <boost/log/trivial.hpp>
#include <iostream>
#include "ui/panel.hpp"
#include <boost/thread/thread.hpp>
#include <chrono>

namespace Panel{
void FakeDevice::set_backlight(bool is_powered)
{
}

void FakeDevice::click()
{
}

Input::bs5_damage_t FakeDevice::read_status()
{
	Panel::Input::bs5_damage_t d = { false, false, false, false };
	bs5_message_t x = { 0,0,0,0,0,0 };
	boost::this_thread::sleep_for((boost::chrono::milliseconds)1);
	switch (this->phase) {
	case 0:
		this->phase = 4;
		break;
		x.wheel_1 += 2;
		d.wheel_1 = true;
		if (this->input.bs5_state.posWheel1 >= 1000) this->phase++;
		break;
	case 1:
		x.wheel_1 -= 2;
		d.wheel_1 = true;
		if (this->input.bs5_state.posWheel1 <= 0) this->phase++;
		break;
	case 2:
		x.wheel_2 += 2;
		d.wheel_2 = true;
		if (this->input.bs5_state.posWheel2 >= 1000) this->phase++;
		break;
	case 3:
		x.wheel_2 -= 2;
		d.wheel_2 = true;
		if (this->input.bs5_state.posWheel2 <= 0) this->phase++;
		break;
	case 4:
		d.wheel_3 = true;
		if (this->input.bs5_state.posWheel3 >= 126) this->phase++;
		x.wheel_3 = this->input.bs5_state.posWheel3 + 1;
		break;
	case 5:
		d.wheel_3 = true;
		if (this->input.bs5_state.posWheel3 <= 1)
			this->phase = 4;
		x.wheel_3 = this->input.bs5_state.posWheel3 - 1;
		break;
	default:
		d.buttons = true;
		x.buttons |= 0x40;
		break;
	}
	this->input.parse_message(x);
	return d;
}
}