#include "modules.hpp"
#include "test.hpp"
#include "../ui/ui.hpp"
#include <iostream>

ModuleManager::ModuleManager() {
    TestModule test;
    this->module_list.push_front(test);
};

ui_module_list_t ModuleManager::get_ui_modules() {
    ui_module_list_t list;
    for(auto module: this->module_list) {
        if (module.ui_module != nullptr) {
            std::cout << "not nullptr\n";
            list.push_front(*module.ui_module);
        } else std::cout << "skipping nullptr\n";
    }
    return list;
}

