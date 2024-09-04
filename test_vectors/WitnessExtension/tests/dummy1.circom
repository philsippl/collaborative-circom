pragma circom 2.0.0;

template Example () {
    signal input a[3];
    signal output c[3];
    
    for (var i=0;i<3;i++) {
        if (i < 2) {
            c[i] <== a[i] * 2;
        } else {
            c[i] <== a[i];
        }
    }
}

component main { public [ a ] } = Example();