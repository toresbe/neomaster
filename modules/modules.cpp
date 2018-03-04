#include "modules/modules.hpp"
#include "test.hpp"
#include "../ui/ui.hpp"
#include <iostream>
#include <boost/log/trivial.hpp>

ModuleManager::ModuleManager() {
	this->module_list.push_front(new TestModule);
	this->module_list.push_front(new TestModule);
	this->module_list.push_front(new TestModule);
}

void NeomasterModuleUI::attach_ui(NeomasterUI * ui)
{
	this->ui = ui;
	this->initialize();
}