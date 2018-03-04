#ifndef TEST_HPP

#define TEST_HPP

#include "modules/modules.hpp"
#include "../ui/ui.hpp"
#include <string>
#include <list>

class TestModuleUI : public NeomasterModuleUI {
public:
	TestModuleUI();
	void initialize();
	void show();
	void draw();
	void handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state);
	int active_color = 0;
	std::list<NMWidget *> widget_list;
	NMProgressBar *bar;
	float value = 0;
};

class TestModule: public NeomasterModule {
    public:
        TestModule();
		NeomasterModuleUI *get_ui_module();
		TestModuleUI * ui_module;
};


#endif
