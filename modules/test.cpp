#include <iostream>
#include <string>
#include "ui/ui.hpp"
#include "test.hpp"
#include <boost/log/trivial.hpp>
#include "modules/modules.hpp"
#include "modules/test.hpp"
#include "ui/widgets.hpp"
#include "ui/gui.hpp"

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
	SDL_SetRenderDrawColor(this->ui->renderer, 255, 0, 255, 255);
	SDL_RenderClear(this->ui->renderer);
	this->value += 0.003;
	if (this->value >= 1) this->value = 0;
	this->bar->set_value(this->value);
	for (auto widget : this->widget_list) {
		widget->draw();
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
