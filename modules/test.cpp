#include <iostream>
#include <string>
#include <array>
#include "test.hpp"
#include <boost/log/trivial.hpp>
#include "modules/modules.hpp"
#include "modules/test.hpp"
#include "ui/gui.hpp"

namespace Modules {
	namespace Test {
		typedef struct test_rgb { uint8_t r; uint8_t g; uint8_t b; } test_rgb;

		std::array<test_rgb, 4> color_list = {
			test_rgb{ 218,165,32 },
			test_rgb{ 184,134,11 },
			test_rgb{ 124,252,0 },
			test_rgb{ 186,85,211 },
		};

		ModuleUI::ModuleUI() {
			this->description = "Test module";
			this->label = "TEST";
			// It feels like bad practice to have this much code in my constructor?
			// But I don't quite know a better place to put it yet.
		}

		void ModuleUI::initialize() {
			this->bar = new Widget::NMProgressBar(this->ui, 200, 400, 150, 40);
			this->widget_list.push_front(bar);
		}


		void ModuleUI::show() {

		}

		void ModuleUI::draw()
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

		void ModuleUI::handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state)
		{
			if (damage.buttons)
				if (state.button_right) {
					this->active_color++;
					if (color_list.size() <= this->active_color) this->active_color = 0;
				}
				else if (state.button_left) {
					this->active_color--;
					if (this->active_color == 0) this->active_color = color_list.size();
				}
		}

		NeomasterModuleUI * Module::get_ui_module()
		{
			return this->ui_module;
		}

		Module::Module() {
			this->ui_module = new ModuleUI;
			BOOST_LOG_TRIVIAL(info) << "Test module has been instantiated";
		};
	}
}
