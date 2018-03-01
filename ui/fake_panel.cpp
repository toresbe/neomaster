#include <boost/log/trivial.hpp>
#include <iostream>
#include "ui/panel.hpp"
#include <boost/thread/thread.hpp>
#include <chrono>


void FakeBS5panel::set_backlight(bool is_powered)
{
}

void FakeBS5panel::click()
{
}

bs5_message_t FakeBS5panel::read_status()
{
	bs5_message_t x = { 0,0,0,0,0,0 };
	boost::this_thread::sleep_for((boost::chrono::milliseconds)1);
	switch (this->phase) {
	case 0:
		x.wheel_1+=2;
		if (this->input.bs5_state.posWheel1 >= 1000) this->phase++;
		break;
	case 1:
		x.wheel_1-=2;
		if (this->input.bs5_state.posWheel1 <= 0) this->phase++;
		break;
	case 2:
		x.wheel_2 += 2;
		if (this->input.bs5_state.posWheel2 >= 1000) this->phase++;
		break;
	case 3:
		x.wheel_2 -= 2;
		if (this->input.bs5_state.posWheel2 <= 0) this->phase++;
		break;
	case 4:
		x.wheel_3 = this->input.bs5_state.posWheel3 + 1;
		if (this->input.bs5_state.posWheel3 >= 0x7f) this->phase++;
		break;
	case 5:
		x.wheel_3 = this->input.bs5_state.posWheel3 - 1;
		if (this->input.bs5_state.posWheel3 <= 0) this->phase = 0;
		break;
	default:
		x.buttons |= 0x40;
		break;
	}
	this->input.parse_message(x);
	return x;
}
