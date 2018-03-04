#ifndef TEST_HPP

#define TEST_HPP

#include "modules/modules.hpp"
#include "../ui/ui.hpp"
#include <string>
#include <list>

namespace Modules {
	namespace Test {
		class ModuleUI : public NeomasterModuleUI {
		public:
			ModuleUI();
			void initialize();
			void show();
			void draw();
			void handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state);
			int active_color = 0;
			std::list<Widget::NMWidget *> widget_list;
			Widget::NMProgressBar *bar;
			float value = 0;
		};

		class Module : public NeomasterModule {
		public:
			Module();
			NeomasterModuleUI *get_ui_module();
			ModuleUI * ui_module;
		};
	}
}
#endif