#include <iostream>
#include <string>
#include <array>
#include "ui/ui.hpp"
#include "test.hpp"
#include <boost/log/trivial.hpp>
#include "modules/modules.hpp"
#include "modules/test.hpp"
#include "ui/widgets.hpp"
#include "ui/gui.hpp"
#include "ui/panel.hpp"

typedef struct test_rgb { uint8_t r; uint8_t g; uint8_t b; } test_rgb;
std::array<test_rgb, 4> color_list = {
	test_rgb{ 218,165,32 },
	test_rgb{ 184,134,11 },
	test_rgb{ 124,252,0 },
	test_rgb{ 186,85,211 },
};


TestModuleUI::TestModuleUI() {
	this->description = "Test module";
	this->label = "TEST";
	// It feels like bad practice to have this much code in my constructor?
	// But I don't quite know a better place to put it yet.
}

void TestModuleUI::initialize() {
	this->bar = new NMProgressBar(this->ui, 200, 400, 150, 40);
	this->widget_list.push_front(bar);
}


void TestModuleUI::show() {

}

void TestModuleUI::draw()
{
	auto bgcol = color_list[this->active_color];
	SDL_SetRenderDrawColor(this->ui->renderer, bgcol.r, bgcol.g, bgcol.b, 255);
	SDL_RenderClear(this->ui->renderer);
	this->value += 0.003;
	if (this->value >= 1) this->value = 0;
	this->bar->set_value(this->value);
	for (auto widget : this->widget_list) {
		widget->draw();
	}
}

void TestModuleUI::handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state)
{
	if(damage.buttons)
		if (state.button_right) {
			this->active_color++;
			if (color_list.size() <= this->active_color) this->active_color = 0;
		} else if (state.button_left) {
			this->active_color--;
			if (this->active_color == 0) this->active_color = color_list.size();
		}
}

NeomasterModuleUI * TestModule::get_ui_module()
{
	return this->ui_module;
}

TestModule::TestModule() {
	this->ui_module = new TestModuleUI;
    BOOST_LOG_TRIVIAL(info) << "Test module has been instantiated";
};
